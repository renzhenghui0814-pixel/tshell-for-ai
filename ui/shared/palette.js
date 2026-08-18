/*
 * The window's palette, and what the user changed about it.
 *
 * `theme.css` is the palette that ships: two halves, each about forty-five
 * tokens, of which fifteen are decisions and the rest follow from them. The
 * settings page lets the fifteen be edited; this file is what turns those
 * fifteen into the other thirty and writes the result into a page.
 *
 * Three rules hold the whole design up.
 *
 * ONE: an untouched token emits nothing. The file under `theme.rs` records only
 * what the user changed, and this file only ever writes declarations for those
 * -- everything else falls through to the stylesheet, so retuning the shipped
 * palette in a later build reaches everyone who never opened the panel.
 *
 * TWO: a derived token is emitted only when one of its inputs was changed.
 * `--ac-hover` is not "eight percent lighter than the accent" as a matter of
 * principle -- it is a value somebody chose and measured. Recomputing it from
 * an accent nobody edited would throw that away for an approximation of it. The
 * derivation exists to answer "the accent moved, now what?", and that is the
 * only question it is asked.
 *
 * THREE: the derivation runs here, not in Rust. The settings page has to show
 * the result while a colour is still being dragged, and the shell has to write
 * the same result into every frame. Both are JavaScript; doing it on the other
 * side would mean a round trip per pointer move and a second copy of the
 * shipped palette in a language that cannot read the stylesheet.
 *
 * Loaded by host.js (which applies it to every page, before first paint) and by
 * settings (which edits it and needs the derived half to preview and to check).
 */
(function () {
  'use strict';

  /*
   * The fifteen, in the order the settings page lists them, with the CSS name
   * each one carries. `theme.rs` holds the same list and drops anything not in
   * it -- two copies, because one is the file format's guarantee and the other
   * is this file's, and neither can be the other's authority. They are checked
   * against each other by `scripts/palette-check.mjs`.
   */
  var TOKENS = [
    { key: 'bgInset', css: '--bg-inset', group: 'surface' },
    { key: 'bgBase', css: '--bg-base', group: 'surface' },
    { key: 'bgElev', css: '--bg-elev', group: 'surface' },
    { key: 'bgRaise', css: '--bg-raise', group: 'surface' },
    { key: 'bgRaiseHi', css: '--bg-raise-hi', group: 'surface' },
    { key: 'bgFloat', css: '--bg-float', group: 'surface' },
    { key: 'tx', css: '--tx', group: 'text' },
    { key: 'txDim', css: '--tx-dim', group: 'text' },
    { key: 'txFaint', css: '--tx-faint', group: 'text' },
    { key: 'ac', css: '--ac', group: 'accent' },
    { key: 'ok', css: '--ok', group: 'meaning' },
    { key: 'warn', css: '--warn', group: 'meaning' },
    { key: 'err', css: '--err', group: 'meaning' },
    { key: 'info', css: '--info', group: 'meaning' },
    { key: 'ai', css: '--ai', group: 'meaning' }
  ];

  var CSS_OF = {};
  TOKENS.forEach(function (token) { CSS_OF[token.key] = token.css; });

  /*
   * What follows from what.
   *
   * `from` is the base tokens a rule reads, and it is also when the rule fires:
   * change the accent and the eight things that are made of the accent change
   * with it; change nothing and none of them are written at all.
   *
   * The alphas differ between the halves because they are doing different work
   * on different grounds -- a white veil at 6% on near-black is the same
   * *hairline* as ink at 9% on white, not the same number.
   */
  var DERIVED = [
    { css: '--ac-hover', from: ['ac'], fn: function (b, dark) { return shift(b.ac, dark ? 8 : -8); } },
    { css: '--ac-soft', from: ['ac'], fn: function (b, dark) { return alpha(b.ac, dark ? 0.15 : 0.11); } },
    { css: '--ac-ring', from: ['ac'], fn: function (b, dark) { return alpha(b.ac, dark ? 0.42 : 0.34); } },
    { css: '--ac-tx', from: ['ac'], fn: function (b) { return ink(b.ac); } },
    { css: '--term-cursor', from: ['ac'], fn: function (b) { return b.ac; } },
    { css: '--term-select', from: ['ac'], fn: function (b, dark) { return alpha(b.ac, dark ? 0.32 : 0.22); } },

    { css: '--ok-soft', from: ['ok'], fn: function (b, dark) { return alpha(b.ok, dark ? 0.16 : 0.13); } },
    { css: '--warn-soft', from: ['warn'], fn: function (b, dark) { return alpha(b.warn, dark ? 0.16 : 0.14); } },
    { css: '--err-hover', from: ['err'], fn: function (b, dark) { return shift(b.err, dark ? 8 : -8); } },
    { css: '--err-soft', from: ['err'], fn: function (b, dark) { return alpha(b.err, dark ? 0.15 : 0.12); } },
    { css: '--err-tx', from: ['err'], fn: function (b) { return ink(b.err); } },

    /*
     * The furniture: lines, hovers, the scrollbar. Made of the text colour
     * rather than of black or white, which is what the stylesheet uses. The
     * difference only shows when somebody tints their text -- and then it is
     * the difference between furniture that belongs to the palette and
     * furniture that was left behind by it.
     */
    { css: '--line', from: ['tx'], fn: function (b, dark) { return alpha(b.tx, dark ? 0.06 : 0.09); } },
    { css: '--line-strong', from: ['tx'], fn: function (b, dark) { return alpha(b.tx, dark ? 0.11 : 0.14); } },
    { css: '--hover', from: ['tx'], fn: function (b, dark) { return alpha(b.tx, dark ? 0.055 : 0.045); } },
    { css: '--active', from: ['tx'], fn: function (b, dark) { return alpha(b.tx, dark ? 0.09 : 0.08); } },
    { css: '--scroll', from: ['txDim'], fn: function (b, dark) { return alpha(b.txDim, dark ? 0.30 : 0.28); } },
    { css: '--scroll-hover', from: ['txDim'], fn: function (b, dark) { return alpha(b.txDim, dark ? 0.52 : 0.48); } }
  ];

  // -- colour arithmetic -----------------------------------------------------

  /** `#RGB` or `#RRGGBB`, either case, to three 0-255 numbers. Null if it is not one. */
  function rgb(value) {
    if (typeof value !== 'string') return null;
    var body = value.trim().replace(/^#/, '');
    if (body.length === 3) body = body.charAt(0) + body.charAt(0) + body.charAt(1) + body.charAt(1) + body.charAt(2) + body.charAt(2);
    if (!/^[0-9a-fA-F]{6}$/.test(body)) return null;
    return [
      parseInt(body.slice(0, 2), 16),
      parseInt(body.slice(2, 4), 16),
      parseInt(body.slice(4, 6), 16)
    ];
  }

  function hex(parts) {
    return '#' + parts.map(function (n) {
      var clamped = Math.max(0, Math.min(255, Math.round(n)));
      return (clamped < 16 ? '0' : '') + clamped.toString(16);
    }).join('').toUpperCase();
  }

  function toHsl(parts) {
    var r = parts[0] / 255, g = parts[1] / 255, b = parts[2] / 255;
    var max = Math.max(r, g, b), min = Math.min(r, g, b);
    var l = (max + min) / 2;
    var d = max - min;
    if (!d) return [0, 0, l * 100];
    var s = d / (1 - Math.abs(2 * l - 1));
    var h;
    if (max === r) h = ((g - b) / d) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
    return [h, s * 100, l * 100];
  }

  function fromHsl(hsl) {
    var h = ((hsl[0] % 360) + 360) % 360;
    var s = Math.max(0, Math.min(100, hsl[1])) / 100;
    var l = Math.max(0, Math.min(100, hsl[2])) / 100;
    var c = (1 - Math.abs(2 * l - 1)) * s;
    var x = c * (1 - Math.abs((h / 60) % 2 - 1));
    var m = l - c / 2;
    var t = h < 60 ? [c, x, 0]
      : h < 120 ? [x, c, 0]
        : h < 180 ? [0, c, x]
          : h < 240 ? [0, x, c]
            : h < 300 ? [x, 0, c]
              : [c, 0, x];
    return [(t[0] + m) * 255, (t[1] + m) * 255, (t[2] + m) * 255];
  }

  /** Lightness, by however many points, keeping the hue. */
  function shift(value, points) {
    var parts = rgb(value);
    if (!parts) return value;
    var hsl = toHsl(parts);
    return hex(fromHsl([hsl[0], hsl[1], hsl[2] + points]));
  }

  function alpha(value, amount) {
    var parts = rgb(value);
    if (!parts) return value;
    return 'rgba(' + parts[0] + ', ' + parts[1] + ', ' + parts[2] + ', ' + amount + ')';
  }

  function luminance(parts) {
    var channels = parts.map(function (n) {
      var c = n / 255;
      return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
  }

  /** WCAG contrast between two opaque colours, 1 to 21. Zero if either is not one. */
  function contrast(a, b) {
    var pa = rgb(a), pb = rgb(b);
    if (!pa || !pb) return 0;
    var la = luminance(pa), lb = luminance(pb);
    var hi = Math.max(la, lb), lo = Math.min(la, lb);
    return (hi + 0.05) / (lo + 0.05);
  }

  /*
   * What to write *on* a filled colour: white if white can be read on it, and a
   * near-black of the same hue if it cannot.
   *
   * This is the rule theme.css arrived at by hand and wrote down twice. On the
   * dark accent white measures 3.57:1, so that pair is dark ink; on the light
   * accent it clears, so that pair is white. Same rule, both answers, and no
   * chance of somebody picking an accent that leaves an unreadable label on
   * every primary button in the window.
   *
   * The dark ink is the colour's own hue at a fraction of its saturation, not
   * plain black: a button whose label is faintly the colour of the button reads
   * as one object, and pure black against an indigo reads as a hole in it.
   */
  function ink(value) {
    if (contrast('#FFFFFF', value) >= 4.5) return '#FFFFFF';
    var parts = rgb(value);
    if (!parts) return '#FFFFFF';
    var hsl = toHsl(parts);
    return hex(fromHsl([hsl[0], Math.min(hsl[1], 30), 6]));
  }

  // -- the shipped palette ---------------------------------------------------

  /*
   * What the stylesheet says, read out of the stylesheet.
   *
   * The settings page needs a value for every one of the fifteen even before
   * anything is edited -- a colour input has to open on something -- and the
   * half being edited is not always the half being displayed, so
   * `getComputedStyle` cannot answer for both. The rules are same-origin, so
   * their declarations can simply be read.
   *
   * Copying the thirty values into this file instead would have been three
   * lines shorter and would have made theme.css stop being where the palette
   * lives, which is the entire point of theme.css.
   */
  var shipped = null;

  function readShipped(doc) {
    /*
     * `css` holds the derived names as written; the rest of the map is the
     * fifteen, keyed the way the file and the settings page key them. One
     * object because they are read out of the same two rules in one pass.
     */
    var out = { dark: { css: {} }, light: { css: {} } };
    var sheets = doc.styleSheets;
    for (var i = 0; i < sheets.length; i += 1) {
      var rules;
      try {
        rules = sheets[i].cssRules;
      } catch (error) {
        continue; // Not ours to read. Nothing here is worth an exception.
      }
      if (!rules) continue;
      // Our own injected block is the user's edits, not what ships. Reading it
      // back would make every edit look like a default the moment it applied.
      if (sheets[i].ownerNode && sheets[i].ownerNode.id === STYLE_ID) continue;
      for (var j = 0; j < rules.length; j += 1) {
        var rule = rules[j];
        if (!rule.selectorText || !rule.style) continue;
        var half = rule.selectorText === ':root' ? 'dark'
          : /^:root\[data-theme=["']?light["']?\]$/.test(rule.selectorText) ? 'light'
            : null;
        if (!half) continue;
        TOKENS.forEach(function (token) {
          var value = rule.style.getPropertyValue(token.css).trim();
          if (value) out[half][token.key] = value.toUpperCase();
        });
        /*
         * The derived names as the stylesheet spells them, kept beside the
         * fifteen. `resolved` shows these rather than recomputing them, which
         * is rule TWO seen from the preview's side: the shipped `--ac-tx` was
         * measured, and drawing an approximation of it next to the real window
         * would make the preview quietly wrong about the case where nothing
         * has been edited at all.
         */
        DERIVED.forEach(function (rule2) {
          var value = rule.style.getPropertyValue(rule2.css).trim();
          if (value) out[half].css[rule2.css] = value;
        });
      }
    }
    // Light inherits every token it does not restate, exactly as the cascade
    // gives it: `:root[data-theme=light]` overrides `:root`, it does not replace it.
    TOKENS.forEach(function (token) {
      if (!out.light[token.key] && out.dark[token.key]) out.light[token.key] = out.dark[token.key];
    });
    DERIVED.forEach(function (rule) {
      if (!out.light.css[rule.css] && out.dark.css[rule.css]) {
        out.light.css[rule.css] = out.dark.css[rule.css];
      }
    });
    return out;
  }

  /** The shipped value of every token in a half, read once and kept. */
  function defaults(mode, doc) {
    if (!shipped) {
      var read = readShipped(doc || document);
      // A stylesheet that has not been parsed yet answers with nothing, and
      // caching that would make every token look like it has no default for the
      // rest of the page's life. Read again next time instead.
      if (Object.keys(read.dark).length > 1) shipped = read;
      else return {};
    }
    return shipped[mode === 'light' ? 'light' : 'dark'];
  }

  /*
   * Hand the shipped palette in rather than reading it from a document.
   *
   * For `scripts/palette-check.mjs`, which runs this file under node against
   * theme.css parsed from disk. Nothing in the window uses it: a page that
   * seeded its own defaults would be a page whose idea of "unchanged" is not
   * the stylesheet's.
   */
  function seed(both) {
    shipped = {
      dark: Object.assign({ css: {} }, both.dark),
      light: Object.assign({ css: {} }, both.light)
    };
  }

  /** What the stylesheet says a derived token is, before anything was edited. */
  function shippedCss(mode, name) {
    var half = defaults(mode);
    return half.css ? half.css[name] : undefined;
  }

  // -- the file --------------------------------------------------------------

  function emptyFile() {
    return { version: 1, font: '', fontMono: '', dark: {}, light: {} };
  }

  /** Whatever Rust handed over, made safe to index into. */
  function normalize(file) {
    var out = emptyFile();
    if (!file || typeof file !== 'object') return out;
    out.font = typeof file.font === 'string' ? file.font : '';
    out.fontMono = typeof file.fontMono === 'string' ? file.fontMono : '';
    ['dark', 'light'].forEach(function (half) {
      var source = file[half];
      if (!source || typeof source !== 'object') return;
      TOKENS.forEach(function (token) {
        var value = source[token.key];
        if (rgb(value)) out[half][token.key] = String(value).trim().toUpperCase();
      });
    });
    return out;
  }

  /*
   * A font stack on its way into a stylesheet.
   *
   * The user types this, and it lands in a declaration, so it is the one value
   * here that is not already constrained to six hex digits. Everything that
   * could end the declaration or the rule comes out, the colon included -- no
   * font stack has one, and leaving it in is the difference between a name the
   * parser ignores and a second declaration. An unbalanced quote takes all the
   * quotes with it, because a stray one swallows the rest of the block.
   */
  function fontValue(raw) {
    if (typeof raw !== 'string') return '';
    var clean = raw.replace(/[;{}<>\\:]/g, '').replace(/\s+/g, ' ').trim();
    var doubles = (clean.match(/"/g) || []).length;
    var singles = (clean.match(/'/g) || []).length;
    if (doubles % 2 || singles % 2) clean = clean.replace(/["']/g, '');
    return clean;
  }

  /**
   * Every declaration one half produces: what was changed, and what follows
   * from what was changed. Also what the settings page previews and measures.
   */
  function derive(file, mode) {
    var half = normalize(file)[mode === 'light' ? 'light' : 'dark'];
    var dark = mode !== 'light';
    var out = {};

    TOKENS.forEach(function (token) {
      if (half[token.key]) out[token.css] = half[token.key];
    });

    DERIVED.forEach(function (rule) {
      var touched = rule.from.some(function (key) { return !!half[key]; });
      if (!touched) return; // Rule TWO: the shipped value was chosen, not computed.
      /*
       * Only the inputs this rule reads, and the shipped palette is consulted
       * only for an input the user left alone. Every rule here reads one token
       * and fires only when that token was edited, so applying a saved palette
       * -- which host.js does inside <head>, before the stylesheet has
       * necessarily been parsed -- never has to read a stylesheet at all.
       */
      var base = {};
      rule.from.forEach(function (key) {
        base[key] = half[key] || defaults(mode)[key];
      });
      out[rule.css] = rule.fn(base, dark);
    });

    return out;
  }

  /**
   * Every token's effective value in a half -- what the window would paint with.
   *
   * The shipped value where nothing was edited, the stylesheet's own spelling of
   * a derived token where the stylesheet has one, and the rule's answer only
   * where the rule actually fired. This is what the settings page previews and
   * measures, so it has to be the same three sources the page itself resolves
   * from and in the same order.
   */
  function resolved(file, mode) {
    var out = {};
    TOKENS.forEach(function (token) { out[token.css] = defaults(mode)[token.key]; });
    DERIVED.forEach(function (rule) {
      var shippedValue = shippedCss(mode, rule.css);
      if (shippedValue) { out[rule.css] = shippedValue; return; }
      // Not in the stylesheet (a rule newer than theme.css, or a document that
      // has none): compute it, which is the answer the window would reach too.
      var base = {};
      rule.from.forEach(function (key) { base[key] = defaults(mode)[key]; });
      out[rule.css] = rule.fn(base, mode !== 'light');
    });
    var changed = derive(file, mode);
    Object.keys(changed).forEach(function (name) { out[name] = changed[name]; });
    return out;
  }

  function block(selector, declarations) {
    var names = Object.keys(declarations);
    if (!names.length) return '';
    return selector + ' {\n' + names.map(function (name) {
      return '  ' + name + ': ' + declarations[name] + ';';
    }).join('\n') + '\n}\n';
  }

  /**
   * Both halves at once, in the shape theme.css uses.
   *
   * Both, and not just the one in force, so that switching the theme needs
   * nothing from this file -- the same cascade that flips the stylesheet flips
   * the edits on top of it. The block is emitted after theme.css in document
   * order, so equal specificity resolves this way.
   */
  function css(file) {
    var clean = normalize(file);
    var dark = derive(clean, 'dark');
    var fonts = {};
    if (fontValue(clean.font)) fonts['--font'] = fontValue(clean.font);
    if (fontValue(clean.fontMono)) fonts['--font-mono'] = fontValue(clean.fontMono);
    // The fonts are not a property of being dark or light, so they ride the
    // unqualified block with the dark edits rather than being written twice.
    Object.keys(fonts).forEach(function (name) { dark[name] = fonts[name]; });
    return block(':root', dark) + block(':root[data-theme="light"]', derive(clean, 'light'));
  }

  var STYLE_ID = 'tshell-palette';

  /** Put the edits into a document, replacing whatever was there before. */
  function apply(doc, file) {
    var target = doc || document;
    var node = target.getElementById(STYLE_ID);
    var text = css(file);
    if (!node) {
      if (!text) return;
      node = target.createElement('style');
      node.id = STYLE_ID;
      (target.head || target.documentElement).appendChild(node);
    }
    node.textContent = text;
  }

  var api = {
    tokens: TOKENS,
    derivedNames: DERIVED.map(function (rule) { return rule.css; }),
    styleId: STYLE_ID,
    cssName: function (key) { return CSS_OF[key]; },
    empty: emptyFile,
    normalize: normalize,
    defaults: defaults,
    seed: seed,
    derive: derive,
    resolved: resolved,
    css: css,
    apply: apply,
    contrast: contrast,
    hex: function (value) { var parts = rgb(value); return parts ? hex(parts) : ''; },
    fontValue: fontValue
  };

  /*
   * A global, like every other shared script here -- the pages have no module
   * loader and no build step. `globalThis` rather than `window` because the
   * check script runs this same file under node, where there is no window and
   * where the whole point is to test what the window will run.
   */
  globalThis.tshellPalette = api;
  if (typeof module === 'object' && module.exports) module.exports = api;
})();
