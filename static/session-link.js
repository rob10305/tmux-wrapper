// Opening a session straight from a link: /?session=<name>. Pure logic — the
// page's effects are passed in — so it can be unit tested under node;
// terminal.js calls start() once on load.
//
// A link only ever attaches to a session that already exists: the name is
// checked against the list from /api/sessions before the terminal connects,
// so a link can't create a session or count against the session cap.
(function (root, factory) {
    if (typeof module === 'object' && module.exports) {
        module.exports = factory();
    } else {
        root.SessionLink = factory();
    }
})(this, function () {
    'use strict';

    // Same rule as is_valid_session_name (src/tmux.rs) and the picker, plus
    // non-empty: a missing name means "no link".
    function isValidSessionName(name) {
        return typeof name === 'string' && /^[A-Za-z0-9_-]+$/.test(name);
    }

    // env:
    //   href            the page's address (location.href)
    //   fetchSessions() resolves to the session list, or null when it has
    //                   already handled an auth failure (sign-in or denied)
    //   connect(name)   attach the terminal to this session
    //   showPicker()    show the session picker as on a normal load
    //   replaceState(u) replace the address bar with u (same origin)
    async function start(env) {
        var url = new URL(env.href);
        if (!url.searchParams.has('session')) {
            env.showPicker();
            return;
        }

        var values = url.searchParams.getAll('session');
        var name = values.length === 1 ? values[0] : null;

        // Drop the parameter so a reload or a later visit doesn't force it.
        function forget() {
            url.searchParams.delete('session');
            env.replaceState(url.pathname + url.search + url.hash);
        }

        if (!isValidSessionName(name)) {
            forget();
            env.showPicker();
            return;
        }

        var sessions = await env.fetchSessions();
        if (sessions === null) {
            // Signing in again reloads this page; keep the link for that.
            return;
        }
        forget();

        var exists = Array.isArray(sessions) && sessions.some(function (s) {
            return s !== null && typeof s === 'object' && s.name === name;
        });
        if (exists) {
            env.connect(name);
        } else {
            env.showPicker();
        }
    }

    return { isValidSessionName: isValidSessionName, start: start };
});
