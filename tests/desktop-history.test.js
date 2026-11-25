'use strict';
// Run with: node --test tests/*.test.js
// The key bar (with its History button) only shows on touch screens, so a
// computer with a mouse or trackpad needs its own way into History.
const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');

const html = fs.readFileSync(path.join(__dirname, '../static/index.html'), 'utf8');
const css = fs.readFileSync(path.join(__dirname, '../static/terminal.css'), 'utf8');
const js = fs.readFileSync(path.join(__dirname, '../static/terminal.js'), 'utf8');

test('a corner History button exists outside the touch-only key bar', () => {
    const keyBar = html.slice(html.indexOf('<div id="key-bar">'), html.indexOf('</div>', html.indexOf('<div id="key-bar">')));
    assert.match(html, /<button id="btn-history-corner"[^>]*>History<\/button>/);
    assert.ok(!keyBar.includes('btn-history-corner'), 'must not live inside the key bar');
});

test('the corner button is hidden by default and shown only for mouse/trackpad', () => {
    assert.match(css, /#btn-history-corner\s*\{[^}]*display:\s*none/);
    const m = css.match(/@media \(hover: hover\) and \(pointer: fine\)\s*\{([\s\S]*?)\n\}/);
    assert.ok(m, 'needs a (hover: hover) and (pointer: fine) block');
    assert.match(m[1], /#btn-history-corner\s*\{[^}]*display:\s*block/);
});

test('the corner button opens History', () => {
    assert.match(js, /getElementById\('btn-history-corner'\)\.addEventListener\('click',[^)]*openHistory/);
});
