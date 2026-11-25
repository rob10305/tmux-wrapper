//! tmux operations, run as the current process user (the helper runs as the
//! target user, so nothing switches users). `socket: Option<&Path>` adds `-S <path>`
//! so tests can use a scratch server; production passes `None`.

use crate::proto::SessionInfo;
use std::io;
use std::path::Path;
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

pub(crate) const TMUX: &str = "/usr/bin/tmux";

/// Session names may only contain [a-zA-Z0-9_-].
pub fn is_valid_session_name(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Tmux-session cap decision: refuse only when the requested name is NEW and
/// the user is already at the cap. Attaching to an existing session is always
/// allowed regardless of the cap.
pub fn refuses_new_session(existing: &[String], requested: &str, cap: usize) -> bool {
    !existing.iter().any(|s| s == requested) && existing.len() >= cap
}

/// What `tmux list-sessions` (run as the user) says about their tmux server.
#[derive(Debug, PartialEq)]
pub enum TmuxServer {
    Running(Vec<String>),
    NotRunning,
    /// The listing failed for some other reason (e.g. permission denied).
    Unknown,
}

pub fn classify_list_sessions(success: bool, stdout: &str, stderr: &str) -> TmuxServer {
    if success {
        return TmuxServer::Running(stdout.lines().map(str::to_string).collect());
    }
    // tmux 3.4: a stale socket says "no server running on <path>"; a missing
    // one says "error connecting to <path> (No such file or directory)".
    let no_socket = stderr.contains("error connecting")
        && (stderr.contains("No such file or directory") || stderr.contains("Connection refused"));
    if stderr.contains("no server running") || no_socket {
        TmuxServer::NotRunning
    } else {
        TmuxServer::Unknown
    }
}

/// Every tmux / systemctl call gives up after this, so a hung tmux makes the
/// helper answer (Refused, or an empty list) well inside the front's 10 s
/// `HelperDead` timeout instead of stalling it into a restart.
pub(crate) const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(5);

/// Run `cmd` to completion, or kill it and fail with `TimedOut` after `limit`.
async fn output_within(mut cmd: Command, limit: Duration) -> io::Result<Output> {
    cmd.kill_on_drop(true);
    match tokio::time::timeout(limit, cmd.output()).await {
        Ok(output) => output,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("subprocess did not finish within {limit:?}"),
        )),
    }
}

fn tmux_command(socket: Option<&Path>) -> Command {
    let mut cmd = Command::new(TMUX);
    if let Some(sock) = socket {
        cmd.arg("-S").arg(sock);
    }
    cmd.env_remove("TMUX");
    cmd
}

/// Ask tmux whether the user's server is up and which sessions it has.
pub async fn query_server(socket: Option<&Path>) -> TmuxServer {
    let mut cmd = tmux_command(socket);
    cmd.args(["list-sessions", "-F", "#{session_name}"]);
    match output_within(cmd, SUBPROCESS_TIMEOUT).await {
        Ok(out) => classify_list_sessions(
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(e) => {
            tracing::warn!(error = %e, "tmux list-sessions failed");
            TmuxServer::Unknown
        }
    }
}

/// Start the user's `tmux-server.service` (D-012), so a server that exited
/// comes back in its clean systemd context. Relies on `XDG_RUNTIME_DIR` and
/// `DBUS_SESSION_BUS_ADDRESS` being set by the helper.
pub async fn start_server_unit() {
    let mut cmd = Command::new("/usr/bin/systemctl");
    cmd.args(["--user", "start", "tmux-server.service"]);
    match output_within(cmd, SUBPROCESS_TIMEOUT).await {
        Ok(out) if out.status.success() => tracing::info!("started tmux-server.service"),
        Ok(out) => tracing::warn!(
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "could not start tmux-server.service"
        ),
        Err(e) => tracing::warn!(error = %e, "failed to run systemctl"),
    }
}

/// Parse `#{session_name}\t#{session_windows}\t#{session_attached}` lines.
fn parse_sessions(stdout: &str) -> Vec<SessionInfo> {
    stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                Some(SessionInfo {
                    name: parts[0].to_string(),
                    windows: parts[1].parse().unwrap_or(0),
                    attached: parts[2] != "0",
                })
            } else {
                None
            }
        })
        .collect()
}

/// The user's sessions; empty when there is no server or tmux fails.
pub async fn list_sessions(socket: Option<&Path>) -> Vec<SessionInfo> {
    let mut cmd = tmux_command(socket);
    cmd.args([
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_windows}\t#{session_attached}",
    ]);
    match output_within(cmd, SUBPROCESS_TIMEOUT).await {
        Ok(out) if out.status.success() => parse_sessions(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if classify_list_sessions(false, "", &stderr) != TmuxServer::NotRunning {
                tracing::warn!(stderr = %stderr.trim(), "tmux list-sessions failed");
            }
            Vec::new()
        }
        Err(e) => {
            tracing::warn!(error = %e, "tmux list-sessions failed");
            Vec::new()
        }
    }
}

/// Kill a session by exact name. `Ok(false)` means it (or the server) was not found.
pub async fn kill_session(name: &str, socket: Option<&Path>) -> io::Result<bool> {
    let mut cmd = tmux_command(socket);
    cmd.args(["kill-session", "-t"]).arg(format!("={name}"));
    let out = output_within(cmd, SUBPROCESS_TIMEOUT).await?;
    if out.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("can't find session")
        || classify_list_sessions(false, "", &stderr) == TmuxServer::NotRunning
    {
        Ok(false)
    } else {
        Err(io::Error::other(format!(
            "tmux kill-session failed: {}",
            stderr.trim()
        )))
    }
}

/// Most history handed to the page: the newest 8 MiB (far more than 20,000
/// lines of a terminal).
pub const MAX_HISTORY: usize = 8 * 1024 * 1024;

/// Most history lines asked of tmux (its own history-limit may be larger).
const MAX_HISTORY_LINES: u32 = 100_000;

/// The session's whole scrollback plus what is on screen, with its colours
/// as terminal escape codes (the page renders them as plain styled text),
/// wrapped lines joined, oldest first. `None` when there is no
/// such session. Read-only: `capture-pane` changes nothing in the session.
pub async fn capture_history(name: &str, socket: Option<&Path>) -> io::Result<Option<Vec<u8>>> {
    let mut cmd = tmux_command(socket);
    // `=name:` is the exact session (never a prefix match), its current
    // window's active pane; `-S -` starts at the oldest history line.
    // `-S -{MAX_HISTORY_LINES}` bounds what tmux prints however large the
    // user's history-limit is; `-T` drops the padding spaces at line ends;
    // `-e` keeps colours and bold/underline as SGR escape codes.
    cmd.args(["capture-pane", "-p", "-e", "-J", "-T", "-S"])
        .arg(format!("-{MAX_HISTORY_LINES}"))
        .arg("-t")
        .arg(format!("={name}:"));
    let out = output_within(cmd, SUBPROCESS_TIMEOUT).await?;
    if out.status.success() {
        return Ok(Some(keep_newest(trim_trailing_blank(out.stdout), MAX_HISTORY)));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("can't find")
        || classify_list_sessions(false, "", &stderr) == TmuxServer::NotRunning
    {
        Ok(None)
    } else {
        Err(io::Error::other(format!(
            "tmux capture-pane failed: {}",
            stderr.trim()
        )))
    }
}

/// Drop the empty screen rows below the last output; end with one newline.
fn trim_trailing_blank(mut text: Vec<u8>) -> Vec<u8> {
    while text.last().is_some_and(|b| b.is_ascii_whitespace()) {
        text.pop();
    }
    if !text.is_empty() {
        text.push(b'\n');
    }
    text
}

/// At most `max` bytes, cut at a line start so no line is shown half; a
/// single line longer than `max` keeps its tail.
fn keep_newest(text: Vec<u8>, max: usize) -> Vec<u8> {
    if text.len() <= max {
        return text;
    }
    let cut = text.len() - max;
    if cut > 0 && text[cut - 1] == b'\n' {
        return text[cut..].to_vec();
    }
    match text[cut..].iter().position(|&b| b == b'\n') {
        Some(i) if cut + i + 1 < text.len() => text[cut + i + 1..].to_vec(),
        _ => text[cut..].to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ScratchTmux, tmux_available};

    #[test]
    fn session_names_allow_alphanumeric_underscore_dash() {
        assert!(is_valid_session_name("main"));
        assert!(is_valid_session_name("my-session_2"));
        assert!(is_valid_session_name("A1"));
    }

    #[test]
    fn session_names_reject_shell_and_path_metacharacters() {
        assert!(!is_valid_session_name("main session"));
        assert!(!is_valid_session_name("../etc"));
        assert!(!is_valid_session_name("a;rm -rf"));
        assert!(!is_valid_session_name("a|b"));
        assert!(!is_valid_session_name("a$b"));
        assert!(!is_valid_session_name("a.b"));
    }

    #[test]
    fn session_names_reject_non_ascii_alphanumerics() {
        // is_alphanumeric() would accept these; the contract is [A-Za-z0-9_-].
        assert!(!is_valid_session_name("café"));
        assert!(!is_valid_session_name("名前"));
    }

    #[test]
    fn new_session_under_cap_is_allowed() {
        let existing = vec!["a".to_string(), "b".to_string()];
        assert!(!refuses_new_session(&existing, "c", 3));
    }

    #[test]
    fn new_session_at_cap_is_refused() {
        let existing = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(refuses_new_session(&existing, "d", 3));
    }

    #[test]
    fn attach_to_existing_session_at_cap_is_allowed() {
        let existing = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(!refuses_new_session(&existing, "b", 3));
    }

    #[test]
    fn listing_with_sessions_means_running() {
        assert_eq!(
            classify_list_sessions(true, "main\nwork\n", ""),
            TmuxServer::Running(vec!["main".into(), "work".into()])
        );
    }

    #[test]
    fn missing_socket_means_not_running() {
        let stderr = "error connecting to /tmp/tmux-1000/default (No such file or directory)\n";
        assert_eq!(
            classify_list_sessions(false, "", stderr),
            TmuxServer::NotRunning
        );
    }

    #[test]
    fn stale_socket_means_not_running() {
        let stderr = "no server running on /tmp/tmux-1000/default\n";
        assert_eq!(
            classify_list_sessions(false, "", stderr),
            TmuxServer::NotRunning
        );
    }

    #[test]
    fn other_failures_are_unknown() {
        let denied = "error connecting to /tmp/tmux-1000/default (Permission denied)\n";
        assert_eq!(
            classify_list_sessions(false, "", denied),
            TmuxServer::Unknown
        );
        let other = "tmux: unknown failure\n";
        assert_eq!(
            classify_list_sessions(false, "", other),
            TmuxServer::Unknown
        );
    }

    #[tokio::test]
    async fn scratch_server_is_queried_listed_and_killed() {
        let Some(t) = ScratchTmux::start("base") else {
            eprintln!("skipping: /usr/bin/tmux not available");
            return;
        };
        let sock = Some(t.socket());
        assert_eq!(
            query_server(sock).await,
            TmuxServer::Running(vec!["base".into()])
        );
        let sessions = list_sessions(sock).await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "base");
        assert_eq!(sessions[0].windows, 1);
        assert!(!sessions[0].attached);

        assert!(t.new_detached("other"));
        assert!(kill_session("other", sock).await.unwrap());
        assert!(!kill_session("other", sock).await.unwrap());
        assert!(!kill_session("ba", sock).await.unwrap(), "exact match only");
        assert_eq!(list_sessions(sock).await.len(), 1);
    }

    #[tokio::test]
    async fn nonexistent_socket_means_not_running() {
        if !tmux_available() {
            eprintln!("skipping: /usr/bin/tmux not available");
            return;
        }
        let sock = std::env::temp_dir().join("tmuxwrapper-test-no-such-socket");
        assert_eq!(query_server(Some(&sock)).await, TmuxServer::NotRunning);
        assert!(list_sessions(Some(&sock)).await.is_empty());
        assert!(!kill_session("x", Some(&sock)).await.unwrap());
    }

    #[tokio::test]
    async fn a_subprocess_past_its_timeout_is_killed_and_reported() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = std::time::Instant::now();
        let err = output_within(cmd, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn a_subprocess_within_its_timeout_returns_its_output() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hi"]);
        let out = output_within(cmd, Duration::from_secs(5)).await.unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hi\n");
    }

    /// A tmux server that never answers (SIGSTOPped) must not stall the
    /// helper: every operation gives up after the subprocess timeout.
    #[tokio::test]
    async fn a_hung_tmux_server_times_out_instead_of_stalling() {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        let Some(t) = ScratchTmux::start("base") else {
            eprintln!("skipping: /usr/bin/tmux not available");
            return;
        };
        let out = std::process::Command::new(TMUX)
            .env_remove("TMUX")
            .arg("-S")
            .arg(t.socket())
            .args(["display-message", "-p", "#{pid}"])
            .output()
            .unwrap();
        let pid = Pid::from_raw(String::from_utf8_lossy(&out.stdout).trim().parse().unwrap());
        /// Resumes the server before `t` drops (kill-server would hang).
        struct Resume(Pid);
        impl Drop for Resume {
            fn drop(&mut self) {
                let _ = kill(self.0, Signal::SIGCONT);
            }
        }
        kill(pid, Signal::SIGSTOP).unwrap();
        let _resume = Resume(pid);

        let sock = Some(t.socket());
        let started = std::time::Instant::now();
        let (server, sessions, killed) = tokio::time::timeout(Duration::from_secs(8), async {
            tokio::join!(
                query_server(sock),
                list_sessions(sock),
                kill_session("base", sock)
            )
        })
        .await
        .expect("tmux operations stalled on a hung server");
        assert_eq!(server, TmuxServer::Unknown);
        assert!(sessions.is_empty());
        assert!(killed.is_err());
        assert!(started.elapsed() >= SUBPROCESS_TIMEOUT);
    }

    #[test]
    fn parse_sessions_reads_tab_separated_fields() {
        let s = parse_sessions("a\t2\t1\nb\t1\t0\nbroken\n");
        assert_eq!(
            s,
            vec![
                SessionInfo {
                    name: "a".into(),
                    windows: 2,
                    attached: true
                },
                SessionInfo {
                    name: "b".into(),
                    windows: 1,
                    attached: false
                },
            ]
        );
    }

    /// A session on the scratch server running `cmd`, sized small so most
    /// output scrolls into history.
    fn session_running(t: &ScratchTmux, name: &str, cmd: &str) {
        let ok = std::process::Command::new(TMUX)
            .env_remove("TMUX")
            .arg("-S")
            .arg(t.socket())
            .args(["new-session", "-d", "-s", name, "-x", "80", "-y", "10", cmd])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "could not start {name}");
    }

    async fn history_when(t: &ScratchTmux, name: &str, done: &str) -> Vec<u8> {
        for _ in 0..60 {
            if let Ok(Some(out)) = capture_history(name, Some(t.socket())).await
                && String::from_utf8_lossy(&out).contains(done)
            {
                return out;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{name} never showed {done:?}");
    }

    #[tokio::test]
    async fn history_holds_scrolled_off_lines_in_order_without_trailing_blanks() {
        let Some(t) = ScratchTmux::start("base") else {
            return;
        };
        session_running(&t, "hist", "seq 1 300; echo END; sleep 30");
        let out = history_when(&t, "hist", "END").await;
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "1", "starts at the oldest line");
        assert!(text.contains("\n150\n"));
        let tail = &text[text.len() - 20..];
        assert!(text.ends_with("END\n"), "blank screen rows are trimmed: {tail:?}");
    }

    #[tokio::test]
    async fn history_keeps_the_sessions_colours() {
        let Some(t) = ScratchTmux::start("base") else {
            return;
        };
        session_running(&t, "hist", "printf 'plain \\033[31mRED\\033[0m done\\n'; sleep 30");
        let out = history_when(&t, "hist", "done").await;
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("\x1b[31mRED"), "colour codes kept: {text:?}");
    }

    #[tokio::test]
    async fn history_names_the_exact_session() {
        let Some(t) = ScratchTmux::start("base") else {
            return;
        };
        session_running(&t, "hist2", "echo SECOND; sleep 30");
        session_running(&t, "hist", "echo FIRST; sleep 30");
        let out = history_when(&t, "hist", "FIRST").await;
        assert!(!String::from_utf8_lossy(&out).contains("SECOND"));
    }

    #[tokio::test]
    async fn history_of_a_missing_session_is_none() {
        let Some(t) = ScratchTmux::start("base") else {
            return;
        };
        assert_eq!(capture_history("nope", Some(t.socket())).await.unwrap(), None);
    }

    #[test]
    fn long_history_keeps_the_newest_whole_lines() {
        let text = b"aaaa\nbbbb\ncccc\n".to_vec();
        // 9 bytes would cut "bbbb" in half: start at the next whole line.
        assert_eq!(keep_newest(text.clone(), 9), b"cccc\n".to_vec());
        assert_eq!(keep_newest(text.clone(), 10), b"bbbb\ncccc\n".to_vec());
        assert_eq!(keep_newest(text.clone(), 100), text);
        // One huge line: keep its tail rather than nothing.
        assert_eq!(keep_newest(b"0123456789".to_vec(), 4), b"6789".to_vec());
    }

    #[test]
    fn trailing_blank_lines_are_trimmed() {
        assert_eq!(trim_trailing_blank(b"a\nb\n\n   \n\n".to_vec()), b"a\nb\n".to_vec());
        assert_eq!(trim_trailing_blank(b"\n\n".to_vec()), b"".to_vec());
        assert_eq!(trim_trailing_blank(b"a".to_vec()), b"a\n".to_vec());
    }
}
