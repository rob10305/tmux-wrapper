// Colours for the History view: turns the terminal escape codes tmux keeps
// (`capture-pane -e`) into text segments with a style. Pure logic, unit
// tested under node; terminal.js draws each segment as a text node, so no
// session output is ever parsed as HTML. Colours come out only as #rrggbb
// built from numbers, never as text copied from the session.
(function (root, factory) {
    if (typeof module === 'object' && module.exports) {
        module.exports = factory();
    } else {
        root.AnsiText = factory();
    }
})(this, function () {
    'use strict';

    // Same as the live terminal: xterm.js's default colours on black/white.
    var DEFAULT_FG = '#ffffff';
    var DEFAULT_BG = '#000000';
    var BASIC = [
        '#2e3436', '#cc0000', '#4e9a06', '#c4a000', '#3465a4', '#75507b', '#06989a', '#d3d7cf',
        '#555753', '#ef2929', '#8ae234', '#fce94f', '#729fcf', '#ad7fa8', '#34e2e2', '#eeeeec',
    ];
    var CUBE = [0, 95, 135, 175, 215, 255];

    function hex(r, g, b) {
        return '#' + [r, g, b].map(function (v) {
            return (v < 16 ? '0' : '') + v.toString(16);
        }).join('');
    }

    function byte(v) {
        return Number.isInteger(v) && v >= 0 && v <= 255;
    }

    function colour256(n) {
        if (!Number.isInteger(n) || n < 0 || n > 255) return null;
        if (n < 16) return BASIC[n];
        if (n < 232) {
            n -= 16;
            return hex(CUBE[Math.floor(n / 36)], CUBE[Math.floor(n / 6) % 6], CUBE[n % 6]);
        }
        var g = 8 + 10 * (n - 232);
        return hex(g, g, g);
    }

    function plain() {
        return { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, strike: false, inverse: false };
    }

    function isPlain(s) {
        return s.fg === null && s.bg === null && !s.bold && !s.dim && !s.italic &&
            !s.underline && !s.strike && !s.inverse;
    }

    function same(a, b) {
        return a.fg === b.fg && a.bg === b.bg && a.bold === b.bold && a.dim === b.dim &&
            a.italic === b.italic && a.underline === b.underline && a.strike === b.strike &&
            a.inverse === b.inverse;
    }

    // An extended colour (38/48/58) starting at params[i]; returns
    // { colour, next } where next is the index after it.
    function extended(params, i) {
        var p = params[i];
        // Colon form: one param "38:5:n" or "38:2::r:g:b" / "38:2:r:g:b".
        if (p.sub.length > 1) {
            var s = p.sub;
            if (s[1] === 5) return { colour: colour256(s[2]), next: i + 1 };
            if (s[1] === 2) {
                var rgb = s.length >= 6 ? s.slice(3, 6) : s.slice(2, 5);
                return { colour: rgb.every(byte) ? hex(rgb[0], rgb[1], rgb[2]) : null, next: i + 1 };
            }
            return { colour: null, next: i + 1 };
        }
        var mode = params[i + 1] && params[i + 1].sub[0];
        if (mode === 5) {
            return { colour: colour256(params[i + 2] && params[i + 2].sub[0]), next: i + 3 };
        }
        if (mode === 2) {
            var v = [2, 3, 4].map(function (k) { return params[i + k] && params[i + k].sub[0]; });
            return { colour: v.every(byte) ? hex(v[0], v[1], v[2]) : null, next: i + 5 };
        }
        return { colour: null, next: params.length };
    }

    function applySgr(style, paramText) {
        var params = (paramText === '' ? '0' : paramText).split(';').map(function (t) {
            return {
                sub: t.split(':').map(function (x) {
                    return x === '' ? (t === '' ? 0 : undefined) : (/^\d+$/.test(x) ? Number(x) : NaN);
                }),
            };
        });
        var i = 0;
        while (i < params.length) {
            var n = params[i].sub[0];
            if (n === 0) { style = plain(); }
            else if (n === 1) { style.bold = true; }
            else if (n === 2) { style.dim = true; }
            else if (n === 3) { style.italic = true; }
            else if (n === 4) { style.underline = params[i].sub.length > 1 ? params[i].sub[1] !== 0 : true; }
            else if (n === 7) { style.inverse = true; }
            else if (n === 9) { style.strike = true; }
            else if (n === 21) { style.underline = true; }
            else if (n === 22) { style.bold = false; style.dim = false; }
            else if (n === 23) { style.italic = false; }
            else if (n === 24) { style.underline = false; }
            else if (n === 27) { style.inverse = false; }
            else if (n === 29) { style.strike = false; }
            else if (n >= 30 && n <= 37) { style.fg = BASIC[n - 30]; }
            else if (n >= 90 && n <= 97) { style.fg = BASIC[n - 90 + 8]; }
            else if (n >= 40 && n <= 47) { style.bg = BASIC[n - 40]; }
            else if (n >= 100 && n <= 107) { style.bg = BASIC[n - 100 + 8]; }
            else if (n === 39) { style.fg = null; }
            else if (n === 49) { style.bg = null; }
            else if (n === 38 || n === 48 || n === 58) {
                var x = extended(params, i);
                if (x.colour !== null) {
                    if (n === 38) style.fg = x.colour;
                    if (n === 48) style.bg = x.colour;
                }
                i = x.next;
                continue;
            }
            i++;
        }
        return style;
    }

    // Escape codes: CSI (ESC [ ... final), OSC (ESC ] ... BEL or ESC \),
    // other ESC sequences (charset selection and the like), or a lone ESC.
    var ESCAPES = /\x1b(?:\[([0-?]*)([ -\/]*)([@-~])|\][^\x07\x1b]*(?:\x07|\x1b\\)|[P^_][^\x1b]*\x1b\\|[ -\/]*[0-Z\\^-~])|\x1b[\s\S]*$/g;

    function parse(text) {
        var segs = [];
        var style = plain();
        var last = 0;
        function emit(chunk) {
            if (!chunk) return;
            var st = isPlain(style) ? null : Object.assign({}, style);
            var prev = segs[segs.length - 1];
            if (prev && (prev.style === null ? st === null : st !== null && same(prev.style, st))) {
                prev.text += chunk;
            } else {
                segs.push({ text: chunk, style: st });
            }
        }
        ESCAPES.lastIndex = 0;
        var m;
        while ((m = ESCAPES.exec(text)) !== null) {
            emit(text.slice(last, m.index));
            last = ESCAPES.lastIndex;
            if (m[3] === 'm' && m[2] === '' && /^[0-9:;]*$/.test(m[1])) {
                style = applySgr(Object.assign({}, style), m[1]);
            }
            if (m[0] === '') ESCAPES.lastIndex++;
        }
        emit(text.slice(last));
        return segs;
    }

    // Inline style properties for a segment's span.
    function css(s) {
        var fg = s.fg;
        var bg = s.bg;
        if (s.inverse) {
            var t = fg || DEFAULT_FG;
            fg = bg || DEFAULT_BG;
            bg = t;
        }
        var out = {};
        if (fg) out.color = fg;
        if (bg) out.backgroundColor = bg;
        if (s.bold) out.fontWeight = 'bold';
        if (s.dim) out.opacity = '0.6';
        if (s.italic) out.fontStyle = 'italic';
        var deco = [];
        if (s.underline) deco.push('underline');
        if (s.strike) deco.push('line-through');
        if (deco.length) out.textDecoration = deco.join(' ');
        return out;
    }

    return { parse: parse, colour256: colour256, css: css };
});
