// The History view: a session's scrollback as an ordinary scrollable page.
// Pure logic — fetch is passed in — so it can be unit tested under node;
// terminal.js shows the result.
(function (root, factory) {
    if (typeof module === 'object' && module.exports) {
        module.exports = factory();
    } else {
        root.HistoryView = factory();
    }
})(this, function () {
    'use strict';

    var RETRY = 'Tap Refresh to try again.';

    function historyUrl(name) {
        return '/api/sessions/' + encodeURIComponent(name) + '/history';
    }

    // Resolves to one of:
    //   { kind: 'text', text }     the history, oldest first
    //   { kind: 'reauth' }         the Access login lapsed: reload to sign in
    //   { kind: 'message', text }  something to show instead
    async function load(fetch, name) {
        var resp;
        try {
            resp = await fetch(historyUrl(name), { redirect: 'manual', cache: 'no-store' });
        } catch (e) {
            return { kind: 'message', text: 'Could not reach the server. ' + RETRY };
        }
        if (resp.type === 'opaqueredirect' || resp.status === 401) {
            return { kind: 'reauth' };
        }
        if (resp.status === 404) return { kind: 'message', text: 'This session has ended.' };
        if (resp.status === 403) return { kind: 'message', text: 'Access denied.' };
        if (!resp.ok) return { kind: 'message', text: 'Could not load the history. ' + RETRY };
        var text;
        try {
            text = await resp.text();
        } catch (e) {
            return { kind: 'message', text: 'Could not reach the server. ' + RETRY };
        }
        if (!text) return { kind: 'message', text: 'Nothing in this session yet.' };
        return { kind: 'text', text: text };
    }

    return { historyUrl: historyUrl, load: load };
});
