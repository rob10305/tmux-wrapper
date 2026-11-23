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
        ['https://t.example/?session=homelab-map', LIST, '/'],
        ['https://t.example/?session=nope', LIST, '/'],
        ['https://t.example/?session=a%3Bb', LIST, '/'],
        ['https://t.example/?a=1&session=main&b=2#h', LIST, '/?a=1&b=2#h'],
        ['https://t.example/?session=main&session=x', LIST, '/'],
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
