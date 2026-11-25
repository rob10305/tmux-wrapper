'use strict';
// Run with: node --test tests/*.test.js
const test = require('node:test');
const assert = require('node:assert');
const { parse, colour256, css } = require('../static/ansi-text.js');

const E = '\x1b[';
const texts = (segs) => segs.map((s) => s.text).join('');

test('plain text is one unstyled segment', () => {
    assert.deepStrictEqual(parse('hello\nworld\n'), [{ text: 'hello\nworld\n', style: null }]);
});

test('basic, bright, 256 and true colours become CSS colours', () => {
    const segs = parse(E + '31mR' + E + '92mG' + E + '38;5;220mY' + E + '38;2;1;2;3mT' + E + '39mD');
    assert.deepStrictEqual(segs.map((s) => [s.text, s.style && s.style.fg]), [
        ['R', '#cc0000'], ['G', '#8ae234'], ['Y', '#ffd700'], ['T', '#010203'], ['D', null],
    ]);
});

test('your typed messages: grey band with white text, as the session shows it', () => {
    const segs = parse(E + '48;5;237m❯ ' + E + '38;5;231mhi there' + E + '39m  ' + E + '49m\n');
    assert.strictEqual(segs[0].style.bg, '#3a3a3a');
    assert.deepStrictEqual([segs[1].text, segs[1].style.fg, segs[1].style.bg], ['hi there', '#ffffff', '#3a3a3a']);
    assert.strictEqual(segs[segs.length - 1].style, null);
});

test('colour carries across line ends until changed (tmux writes it that way)', () => {
    const segs = parse(E + '32mone\ntwo' + E + '0m');
    assert.deepStrictEqual(segs, [{ text: 'one\ntwo', style: { fg: '#4e9a06', bg: null, bold: false, dim: false, italic: false, underline: false, strike: false, inverse: false } }]);
});

test('bold, dim, italic, underline, strike and their resets', () => {
    const s = parse(E + '1;2;3;4;9mx' + E + '22;23;24;29my')
        .map((g) => g.style);
    assert.deepStrictEqual([s[0].bold, s[0].dim, s[0].italic, s[0].underline, s[0].strike], [true, true, true, true, true]);
    assert.strictEqual(s[1], null);
});

test('colon forms and underline styles that tmux may write', () => {
    const segs = parse(E + '38:2::10:20:30mA' + E + '4:3mB' + E + '4:0mC');
    assert.strictEqual(segs[0].style.fg, '#0a141e');
    assert.strictEqual(segs[1].style.underline, true);
    assert.strictEqual(segs[2].style.underline, false);
});

test('other escape codes (links, cursor moves) are dropped, their text kept', () => {
    const t = '\x1b]8;;https://x.test\x1b\\link\x1b]8;;\x1b\\ a' + E + '2Kb\x1b]0;title\x07c' + E + '?25hd\x1b(Be';
    assert.strictEqual(texts(parse(t)), 'link abcde');
});

test('nothing in the text can become markup or a style injection', () => {
    const segs = parse(E + '38;5;999m<img src=x onerror=alert(1)>' + E + '38;2;300;999;1m"x');
    assert.strictEqual(texts(segs), '<img src=x onerror=alert(1)>"x');
    for (const g of segs) {
        if (g.style && g.style.fg !== null) assert.match(g.style.fg, /^#[0-9a-f]{6}$/);
    }
});

test('a broken escape at the very end does not throw or leak', () => {
    assert.strictEqual(texts(parse('ok' + E + '38;5')), 'ok');
    assert.strictEqual(texts(parse('ok\x1b')), 'ok');
});

test('256-colour table: cube and greys', () => {
    assert.strictEqual(colour256(16), '#000000');
    assert.strictEqual(colour256(231), '#ffffff');
    assert.strictEqual(colour256(246), '#949494');
    assert.strictEqual(colour256(256), null);
});

test('css(): inverse swaps colours, dim fades, defaults fill gaps', () => {
    const base = { fg: null, bg: null, bold: true, dim: true, italic: false, underline: true, strike: true, inverse: true };
    const c = css(base);
    assert.strictEqual(c.color, '#000000');
    assert.strictEqual(c.backgroundColor, '#ffffff');
    assert.strictEqual(c.fontWeight, 'bold');
    assert.strictEqual(c.opacity, '0.6');
    assert.strictEqual(c.textDecoration, 'underline line-through');
});

test('css(): no colour set leaves the page default colour', () => {
    const c = css({ fg: null, bg: null, bold: true, dim: false, italic: false, underline: false, strike: false, inverse: false });
    assert.deepStrictEqual(c, { fontWeight: 'bold' });
});

test('the History page loads the colour reader and draws segments as text only', () => {
    const fs = require('node:fs');
    const path = require('node:path');
    const html = fs.readFileSync(path.join(__dirname, '../static/index.html'), 'utf8');
    const js = fs.readFileSync(path.join(__dirname, '../static/terminal.js'), 'utf8');
    assert.ok(html.indexOf('/ansi-text.js') !== -1 && html.indexOf('/ansi-text.js') < html.indexOf('/terminal.js'));
    assert.match(js, /AnsiText\.parse\(result\.text\)/);
    assert.ok(!/innerHTML/.test(js.slice(js.indexOf('function showHistory'), js.indexOf('function openHistory'))));
});
