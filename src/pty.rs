use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::libc;
use nix::pty::openpty;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A PTY master received from the helper. The tmux client on the other side
/// belongs to the helper, which reaps it; dropping this closes the master,
/// which hangs the client up.
pub struct PtyMaster {
    async_fd: AsyncFd<OwnedFd>,
}

impl PtyMaster {
    /// Wrap an already-open PTY master (e.g. received from the helper). Must
    /// be called within a tokio runtime.
    pub fn from_fd(fd: OwnedFd) -> io::Result<PtyMaster> {
        set_nonblocking(&fd)?;
        // SAFETY: the `OwnedFd` is a valid open descriptor that the `AsyncFd`
        // now owns; nothing else closes or replaces it while it is registered.
        let async_fd = unsafe { AsyncFd::register(fd) }?;
        Ok(PtyMaster { async_fd })
    }

    /// Set the terminal size via TIOCSWINSZ on the master.
    pub fn resize(&self, cols: u16, rows: u16) {
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe { libc::ioctl(self.raw_fd(), libc::TIOCSWINSZ, &ws) };
    }

    pub fn raw_fd(&self) -> RawFd {
        self.async_fd.get_ref().as_raw_fd()
    }
}

fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(io::Error::from)?;
    let flags = OFlag::from_bits_retain(flags) | OFlag::O_NONBLOCK;
    fcntl(fd, FcntlArg::F_SETFL(flags)).map_err(io::Error::from)?;
    Ok(())
}

/// Where a NEW session starts: its project folder `~/lab/<session>` when that
/// exists, else `~/lab`, else the home folder. An existing session is only
/// attached (`-A` ignores `-c`), so this never moves one. The name has already
/// passed is_valid_session_name, so it cannot climb out of `~/lab`.
pub fn start_dir(home: &Path, session: &str) -> PathBuf {
    let lab = home.join("lab");
    let project = lab.join(session);
    if project.is_dir() {
        project
    } else if lab.is_dir() {
        lab
    } else {
        home.to_path_buf()
    }
}

/// Spawn `tmux new-session -A -s <session>` attached to a fresh PTY, as the
/// current user. Returns the PTY master and the client process.
/// Must be called from within a tokio runtime. Dropping the master hangs the
/// client up, so `child.wait()` then completes.
pub fn spawn_tmux_client(
    session: &str,
    home: &Path,
    socket: Option<&Path>,
) -> io::Result<(OwnedFd, tokio::process::Child)> {
    let pty = openpty(None, None).map_err(io::Error::from)?;
    let (master, slave) = (pty.master, pty.slave);
    // The child must not inherit the master, or dropping ours would never hang it up.
    fcntl(&master, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(io::Error::from)?;
    fcntl(&slave, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(io::Error::from)?;

    let mut cmd = tokio::process::Command::new("/usr/bin/tmux");
    if let Some(sock) = socket {
        cmd.arg("-S").arg(sock);
    }
    cmd.args(["new-session", "-A", "-s", session, "-c"])
        .arg(start_dir(home, session))
        .current_dir(home)
        .env("TERM", "xterm-256color")
        .env_remove("TMUX")
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // Safety: runs in the child before exec; only async-signal-safe calls
    // (setsid, ioctl), no allocation, locks or std I/O.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn();
    // Drop the Command (and the parent's slave copies it owns) before returning,
    // on success and failure alike.
    drop(cmd);
    Ok((master, child?))
}

// Reads and writes need only `&self` (AsyncFd tracks read and write readiness
// separately), so the bridge can share one master between its reader, its
// writer and resizes via `&PtyMaster`.
impl PtyMaster {
    fn poll_read_shared(
        &self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = match self.async_fd.poll_read_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };

            let fd = self.async_fd.get_ref().as_raw_fd();
            let unfilled = buf.initialize_unfilled();
            let n = unsafe {
                libc::read(
                    fd,
                    unfilled.as_mut_ptr() as *mut libc::c_void,
                    unfilled.len(),
                )
            };

            if n > 0 {
                buf.advance(n as usize);
                return Poll::Ready(Ok(()));
            } else if n == 0 {
                return Poll::Ready(Ok(()));
            } else {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                }
                return Poll::Ready(Err(err));
            }
        }
    }

    fn poll_write_shared(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = match self.async_fd.poll_write_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };

            let fd = self.async_fd.get_ref().as_raw_fd();
            let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };

            if n >= 0 {
                return Poll::Ready(Ok(n as usize));
            } else {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                }
                return Poll::Ready(Err(err));
            }
        }
    }
}

impl AsyncRead for PtyMaster {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll_read_shared(cx, buf)
    }
}

impl AsyncRead for &PtyMaster {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll_read_shared(cx, buf)
    }
}

impl AsyncWrite for PtyMaster {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_shared(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for &PtyMaster {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_shared(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::{PtyMaster, spawn_tmux_client, start_dir};
    use crate::test_support::ScratchTmux;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    /// A fresh fake home folder for one test.
    fn fake_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tmuxwrapper-home-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_session_starts_in_its_lab_project_folder() {
        let home = fake_home("project");
        std::fs::create_dir_all(home.join("lab/vision")).unwrap();
        assert_eq!(start_dir(&home, "vision"), home.join("lab/vision"));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn without_a_project_folder_it_starts_in_lab() {
        let home = fake_home("lab");
        std::fs::create_dir_all(home.join("lab/other")).unwrap();
        assert_eq!(start_dir(&home, "vision"), home.join("lab"));
        // A file with the session's name is not a project folder.
        std::fs::write(home.join("lab/notes"), "x").unwrap();
        assert_eq!(start_dir(&home, "notes"), home.join("lab"));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn without_lab_it_starts_in_home() {
        let home = fake_home("nolab");
        assert_eq!(start_dir(&home, "vision"), home);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[tokio::test]
    async fn the_spawned_session_opens_in_the_project_folder() {
        let Some(t) = ScratchTmux::start("base") else {
            eprintln!("skipping: /usr/bin/tmux not available");
            return;
        };
        let home = fake_home("spawn");
        std::fs::create_dir_all(home.join("lab/proj")).unwrap();
        let (master, mut child) = spawn_tmux_client("proj", &home, Some(t.socket())).unwrap();
        let mut pty = PtyMaster::from_fd(master).unwrap();
        let mut buf = [0u8; 4096];
        let _ = tokio::time::timeout(Duration::from_secs(2), pty.read(&mut buf)).await;
        // The client creates the session a moment after its first output.
        for _ in 0..40 {
            let names = crate::tmux::list_sessions(Some(t.socket())).await;
            if names.iter().any(|s| s.name == "proj") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // session_path is the folder `-c` set; the pane's own folder can still
        // read as the server's for a moment while its shell starts.
        let out = tokio::process::Command::new("/usr/bin/tmux")
            .arg("-S")
            .arg(t.socket())
            .env_remove("TMUX")
            .args(["display", "-p", "-t", "proj", "#{session_path}"])
            .output()
            .await
            .unwrap();
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(PathBuf::from(path), home.join("lab/proj"));
        drop(pty);
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[tokio::test]
    async fn tmux_client_gets_a_pty_and_exits_when_master_drops() {
        let Some(t) = ScratchTmux::start("base") else {
            eprintln!("skipping: /usr/bin/tmux not available");
            return;
        };
        let home = std::env::temp_dir();
        let (master, mut child) = spawn_tmux_client("t1", &home, Some(t.socket())).unwrap();
        let mut pty = PtyMaster::from_fd(master).unwrap();

        let mut buf = [0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(2), pty.read(&mut buf))
            .await
            .expect("no output within 2s")
            .unwrap();
        assert!(n > 0, "expected terminal output from tmux client");

        pty.resize(100, 30);

        let sessions = crate::tmux::list_sessions(Some(t.socket())).await;
        assert!(sessions.iter().any(|s| s.name == "t1"), "{sessions:?}");

        drop(pty);
        tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .expect("client did not exit after master drop")
            .unwrap();

        assert!(
            crate::tmux::kill_session("t1", Some(t.socket()))
                .await
                .unwrap()
        );
        assert!(
            !crate::tmux::kill_session("t1", Some(t.socket()))
                .await
                .unwrap()
        );
    }
}
