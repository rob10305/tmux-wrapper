'use strict';
// Run with: node --test tests/*.test.js
const test = require('node:test');
const assert = require('node:assert');
const { isValidSessionName, start } = require('../static/session-link.js');

// A fake page: records what start() asked the page to do.
function page(href, sessions) {
    const calls = { connected: [], picker: 0, replaced: [], fetched: 0 };
    const env = {
        href: href,
        fetchSessions: async function () {
            calls.fetched++;
            return sessions;
        },
        connect: function (name) { calls.connected.push(name); },
        showPicker: function () { calls.picker++; },
        replaceState: function (url) { calls.replaced.push(url); },
    };
    return { env, calls };
}

const LIST = [
    { name: 'homelab-map', windows: 1, attached: false },
    { name: 'main', windows: 2, attached: true },
];

test('the name rule matches is_valid_session_name, non-empty', () => {
    for (const ok of ['main', 'my-session_2', 'A1', 'homelab-map']) {
        assert.ok(isValidSessionName(ok), ok);
    }
    for (const bad of ['', '../x', 'a;b', 'main session', 'a/b', 'a.b', 'é', 'a\n', null, undefined, 42]) {
        assert.ok(!isValidSessionName(bad), JSON.stringify(bad));
    }
});

test('a valid, existing session opens directly', async () => {
    const { env, calls } = page('https://t.example/?session=homelab-map', LIST);
    await start(env);
    assert.deepStrictEqual(calls.connected, ['homelab-map']);
    assert.strictEqual(calls.picker, 0);
});

test('a valid but missing session shows the picker and creates nothing', async () => {
    const { env, calls } = page('https://t.example/?session=nope', LIST);
    await start(env);
    assert.deepStrictEqual(calls.connected, []);
    assert.strictEqual(calls.picker, 1);
});

test('an empty or failed session list shows the picker', async () => {
    for (const sessions of [[], undefined, { name: 'main' }, [null, 'main']]) {
        const { env, calls } = page('https://t.example/?session=main', sessions);
        await start(env);
        assert.deepStrictEqual(calls.connected, [], JSON.stringify(sessions));
        assert.strictEqual(calls.picker, 1, JSON.stringify(sessions));
    }
});

test('an invalid name shows the picker without asking the server', async () => {
    for (const raw of ['..%2Fx', 'a%3Bb', '', 'main%20x', '%E2%80%8Bmain']) {
        const { env, calls } = page('https://t.example/?session=' + raw, [{ name: 'a;b' }, { name: '../x' }]);
        await start(env);
        assert.deepStrictEqual(calls.connected, [], raw);
        assert.strictEqual(calls.picker, 1, raw);
        assert.strictEqual(calls.fetched, 0, raw);
    }
});

test('no parameter shows the picker and leaves the address alone', async () => {
    const { env, calls } = page('https://t.example/', LIST);
    await start(env);
    assert.deepStrictEqual(calls.connected, []);
    assert.strictEqual(calls.picker, 1);
    assert.deepStrictEqual(calls.replaced, []);
});

test('the parameter is removed from the address after use', async () => {
    const cases = [
        ['https://t.example/?session=homelab-map', LIST, 'https://t.example/'],
        ['https://t.example/?session=nope', LIST, 'https://t.example/'],
        ['https://t.example/?session=a%3Bb', LIST, 'https://t.example/'],
        ['https://t.example/?a=1&session=main&b=2#h', LIST, 'https://t.example/?a=1&b=2#h'],
        ['https://t.example/?session=main&session=x', LIST, 'https://t.example/'],
    ];
    for (const [href, sessions, expected] of cases) {
        const { env, calls } = page(href, sessions);
        await start(env);
        assert.deepStrictEqual(calls.replaced, [expected], href);
    }
});

test('the parameter is removed before the terminal connects', async () => {
    const order = [];
    const { env } = page('https://t.example/?session=main', LIST);
    env.replaceState = function () { order.push('replace'); };
    env.connect = function () { order.push('connect'); };
    await start(env);
    assert.deepStrictEqual(order, ['replace', 'connect']);
});

test('an Access sign-in redirect keeps the link for after login', async () => {
    // fetchSessions returns null when it has already started reauthenticating
    // (or shown "Access denied"); the page reloads through login.
    const { env, calls } = page('https://t.example/?session=main', null);
    await start(env);
    assert.deepStrictEqual(calls.connected, []);
    assert.strictEqual(calls.picker, 0);
    assert.deepStrictEqual(calls.replaced, []);
});

test('a repeated parameter is ambiguous and shows the picker', async () => {
    const { env, calls } = page('https://t.example/?session=main&session=homelab-map', LIST);
    await start(env);
    assert.deepStrictEqual(calls.connected, []);
    assert.strictEqual(calls.picker, 1);
});

test('a path starting with // keeps the address on this site', async () => {
    // "//?..." as a relative URL would point at another host, which
    // replaceState refuses; the full href stays on this origin.
    const { env, calls } = page('https://t.example//?session=main', LIST);
    await start(env);
    assert.deepStrictEqual(calls.replaced, ['https://t.example//']);
    assert.deepStrictEqual(calls.connected, ['main']);
});

test('a failing replaceState still opens the session or the picker', async () => {
    for (const [href, connected, picker] of [
        ['https://t.example/?session=main', ['main'], 0],
        ['https://t.example/?session=nope', [], 1],
        ['https://t.example/?session=a%3Bb', [], 1],
    ]) {
        const { env, calls } = page(href, LIST);
        env.replaceState = function () { throw new Error('SecurityError'); };
        await start(env);
        assert.deepStrictEqual(calls.connected, connected, href);
        assert.strictEqual(calls.picker, picker, href);
    }
});

// The Create box: spaces become '-', anything else it can't use is explained,
// never silently ignored.
const { newSessionName } = require('../static/session-link.js');

test('a plain name is kept as typed', () => {
    assert.deepStrictEqual(newSessionName('vision'), { name: 'vision' });
    assert.deepStrictEqual(newSessionName('my-session_2'), { name: 'my-session_2' });
});

test('spaces become a single dash, ends are trimmed', () => {
    assert.deepStrictEqual(newSessionName('my project'), { name: 'my-project' });
    assert.deepStrictEqual(newSessionName('  my   new  project '), { name: 'my-new-project' });
    assert.deepStrictEqual(newSessionName('a\tb'), { name: 'a-b' });
    // iPhone non-breaking space from the keyboard counts as a space too
    assert.deepStrictEqual(newSessionName('my project'), { name: 'my-project' });
});

test('an empty name asks for one', () => {
    for (const empty of ['', '   ', null, undefined]) {
        const r = newSessionName(empty);
        assert.strictEqual(r.name, undefined, JSON.stringify(empty));
        assert.strictEqual(r.error, 'Type a name for the new session.');
    }
});

test('other characters are explained, naming the first one', () => {
    assert.deepStrictEqual(newSessionName('vision.2'), {
        error: 'A name can only use letters, numbers, - and _ (not “.”).',
    });
    assert.deepStrictEqual(newSessionName('my project!'), {
        error: 'A name can only use letters, numbers, - and _ (not “!”).',
    });
    assert.deepStrictEqual(newSessionName('café'), {
        error: 'A name can only use letters, numbers, - and _ (not “é”).',
    });
});

test('every name it returns passes the server rule', () => {
    for (const raw of ['a b', ' x ', 'one  two\tthree', 'A1', 'q_w-e']) {
        const r = newSessionName(raw);
        assert.ok(isValidSessionName(r.name), raw);
    }
});
