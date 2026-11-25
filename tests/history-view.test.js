'use strict';
// Run with: node --test tests/*.test.js
const test = require('node:test');
const assert = require('node:assert');
const { historyUrl, load } = require('../static/history-view.js');

// A fake fetch: records the call and answers with `answer` (or throws it).
function fetcher(answer) {
    const calls = [];
    const fetch = async function (url, opts) {
        calls.push({ url, opts });
        if (answer instanceof Error) throw answer;
        return answer;
    };
    return { fetch, calls };
}

function reply(status, body, type) {
    return {
        status: status,
        ok: status >= 200 && status < 300,
        type: type || 'basic',
        text: async function () { return body; },
    };
}

test('the address names the session, encoded', () => {
    assert.strictEqual(historyUrl('homelab-map'), '/api/sessions/homelab-map/history');
    assert.strictEqual(historyUrl('a b'), '/api/sessions/a%20b/history');
});

test('text comes back as text, asked for fresh and without following redirects', async () => {
    const f = fetcher(reply(200, 'line 1\nline 2\n'));
    assert.deepStrictEqual(await load(f.fetch, 'main'), { kind: 'text', text: 'line 1\nline 2\n' });
    assert.strictEqual(f.calls[0].url, '/api/sessions/main/history');
    assert.strictEqual(f.calls[0].opts.redirect, 'manual');
    assert.strictEqual(f.calls[0].opts.cache, 'no-store');
});

test('an expired login asks to sign in again', async () => {
    for (const r of [reply(0, '', 'opaqueredirect'), reply(401, '')]) {
        assert.deepStrictEqual(await load(fetcher(r).fetch, 'main'), { kind: 'reauth' });
    }
});

test('each failure gets a plain message', async () => {
    const cases = [
        [reply(404, ''), 'This session has ended.'],
        [reply(403, ''), 'Access denied.'],
        [reply(503, ''), 'Could not load the history. Tap Refresh to try again.'],
        [reply(500, ''), 'Could not load the history. Tap Refresh to try again.'],
        [new Error('offline'), 'Could not reach the server. Tap Refresh to try again.'],
        [reply(200, ''), 'Nothing in this session yet.'],
    ];
    for (const [answer, text] of cases) {
        assert.deepStrictEqual(await load(fetcher(answer).fetch, 'main'), { kind: 'message', text });
    }
});

test('a body that fails to read is a message, not a crash', async () => {
    const broken = reply(200, '');
    broken.text = async function () { throw new Error('cut off'); };
    assert.deepStrictEqual(await load(fetcher(broken).fetch, 'main'),
        { kind: 'message', text: 'Could not reach the server. Tap Refresh to try again.' });
});
