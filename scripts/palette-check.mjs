/*
 * What the palette promises, checked.
 *
 * The window's colours come from three places that have to agree: theme.css
 * defines them, ui/shared/palette.js derives the two thirds of them that follow
 * from the rest, and src-tauri/src/theme.rs decides which of them a file may
 * carry. Nothing in the running program forces those three to line up -- a
 * token renamed in one and not the others fails silently, as a colour that
 * quietly stops being editable or a declaration nobody reads.
 *
 * So this is the test for the parts that have no test runner. Run it after
 * touching any of the three:
 *
 *     node scripts/palette-check.mjs
 *
 * It checks four things:
 *
 *   1. The three lists of tokens are the same list.
 *   2. The palette that ships clears 4.5:1 everywhere words are drawn -- with
 *      the two exemptions theme.css writes down, and no others.
 *   3. The derivation produces usable colour for accents it has never seen,
 *      which is the whole promise the settings panel makes.
 *   4. An untouched token derives nothing, which is what lets a later build
 *      retune the shipped palette for everyone who never opened the panel.
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (path) => readFileSync(join(root, path), 'utf8');

await import(pathToFileURL(join(root, 'ui/shared/palette.js')).href);
const palette = globalThis.tshellPalette;

let failures = 0;
const fail = (message) => { failures += 1; console.error('  FAIL  ' + message); };
const pass = (message) => console.log('  ok    ' + message);

// -- the stylesheet, parsed ---------------------------------------------------

/*
 * The two `:root` blocks, read for whatever custom properties they declare.
 * Deliberately not a CSS parser: these two blocks are flat lists of
 * `--name: value;` and anything cleverer would be a second thing to maintain.
 */
function block(css, selector) {
  const start = css.indexOf(selector + ' {');
  if (start === -1) throw new Error('theme.css has no ' + selector + ' block');
  const open = css.indexOf('{', start);
  const close = css.indexOf('\n}', open);
  const body = css.slice(open + 1, close);
  const out = {};
  for (const line of body.split('\n')) {
    const match = line.match(/^\s*(--[a-z0-9-]+)\s*:\s*([^;]+);/i);
    if (match) out[match[1]] = match[2].trim();
  }
  return out;
}

const css = read('ui/shared/theme.css');
const shippedDark = block(css, ':root');
const shippedLight = { ...shippedDark, ...block(css, ':root[data-theme="light"]') };

const halves = { dark: {}, light: {} };
for (const token of palette.tokens) {
  halves.dark[token.key] = shippedDark[token.css];
  halves.light[token.key] = shippedLight[token.css];
}
palette.seed(halves);

// -- 1. one list of tokens, in three files ------------------------------------

const rustList = (() => {
  const source = read('src-tauri/src/theme.rs');
  const table = source.match(/const TOKENS: \[&str; \d+\] = \[([^\]]+)\]/);
  if (!table) throw new Error('theme.rs has no TOKENS table');
  return [...table[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
})();

const jsList = palette.tokens.map((token) => token.key);

if (rustList.join(',') !== jsList.join(',')) {
  fail('theme.rs and palette.js disagree about the tokens:\n'
    + '        rust: ' + rustList.join(' ') + '\n'
    + '        js:   ' + jsList.join(' '));
} else {
  pass(jsList.length + ' tokens, the same in theme.rs and palette.js');
}

for (const token of palette.tokens) {
  for (const mode of ['dark', 'light']) {
    if (!halves[mode][token.key]) fail('theme.css has no ' + token.css + ' for ' + mode);
  }
}

// -- 2. the palette that ships ------------------------------------------------

/*
 * Every pair where one of them is words on the other. `--tx-faint` is absent on
 * purpose and theme.css says why: it draws hairlines and icon strokes and
 * disabled controls, which WCAG exempts, and putting words in it is a bug in
 * the caller rather than in the palette.
 */
const PAIRS = [
  ['--tx', '--bg-base'], ['--tx', '--bg-elev'], ['--tx', '--bg-inset'],
  ['--tx', '--bg-raise'], ['--tx', '--bg-float'],
  ['--tx-dim', '--bg-base'], ['--tx-dim', '--bg-elev'], ['--tx-dim', '--bg-float'],
  ['--ac', '--bg-base'], ['--ok', '--bg-base'], ['--warn', '--bg-base'],
  ['--err', '--bg-base'], ['--info', '--bg-base'], ['--ai', '--bg-base'],
  ['--ac-tx', '--ac'], ['--err-tx', '--err']
];

for (const mode of ['dark', 'light']) {
  const shipped = mode === 'dark' ? shippedDark : shippedLight;
  let worst = { ratio: 99, pair: null };
  for (const [ink, ground] of PAIRS) {
    const ratio = palette.contrast(shipped[ink], shipped[ground]);
    if (!ratio) { fail('cannot measure ' + ink + ' on ' + ground + ' (' + mode + ')'); continue; }
    if (ratio < worst.ratio) worst = { ratio, pair: ink + ' on ' + ground };
    if (ratio < 4.5) fail(mode + ': ' + ink + ' on ' + ground + ' is ' + ratio.toFixed(2) + ':1');
  }
  pass(mode + ': all ' + PAIRS.length + ' shipped pairs clear 4.5:1 (worst '
    + worst.ratio.toFixed(2) + ':1, ' + worst.pair + ')');
}

// -- 3. the derivation, on colours it has never seen --------------------------

/*
 * The panel's promise is not "your accent will be used", it is "your accent
 * will be usable" -- the label on a primary button is chosen by rule from the
 * accent underneath it, and the rule has to hold for any accent, not for the
 * two that ship.
 */
const ACCENTS = [
  '#6E7CF7', '#5A5FE0', '#FFFFFF', '#000000', '#F5D046', '#35D6A4',
  '#FF6B6B', '#7A00FF', '#00FFFF', '#808080', '#123456', '#FFC0CB'
];

for (const mode of ['dark', 'light']) {
  let worst = { ratio: 99, accent: null };
  for (const accent of ACCENTS) {
    const full = palette.resolved({ [mode]: { ac: accent, err: accent } }, mode);

    for (const [ink, fill] of [['--ac-tx', '--ac'], ['--err-tx', '--err']]) {
      const ratio = palette.contrast(full[ink], full[fill]);
      if (ratio < worst.ratio) worst = { ratio, accent: accent + ' ' + ink };
      if (ratio < 4.5) {
        fail(mode + ': ' + ink + ' on ' + accent + ' is only ' + ratio.toFixed(2) + ':1');
      }
    }

    // Every derived declaration has to be something CSS can use. A rule that
    // returns its input unchanged on a colour it could not parse would show up
    // here as a token holding a name rather than a colour.
    for (const name of palette.derivedNames) {
      const value = full[name];
      if (!/^(#[0-9A-F]{6}|rgba\(\d+, \d+, \d+, [\d.]+\))$/.test(value)) {
        fail(mode + ': ' + name + ' derived from ' + accent + ' is not a colour: ' + value);
      }
    }
  }
  pass(mode + ': ' + ACCENTS.length + ' accents, ink on every fill clears 4.5:1 (worst '
    + worst.ratio.toFixed(2) + ':1, ' + worst.accent + ')');
}

/*
 * And the rule reproduces the two choices theme.css made by hand: dark ink on
 * the dark theme's accent, white on the light theme's. Not the same hex -- the
 * shipped ones were tuned -- but the same decision, which is the part that
 * would be wrong to get wrong.
 */
for (const [mode, expected] of [['dark', 'dark'], ['light', 'white']]) {
  const shipped = mode === 'dark' ? shippedDark : shippedLight;
  const derived = palette.resolved({ [mode]: { ac: shipped['--ac'] } }, mode)['--ac-tx'];
  const isWhite = (value) => palette.hex(value) === '#FFFFFF';
  const got = isWhite(derived) ? 'white' : 'dark';
  if (got !== expected) {
    fail(mode + ': the ink rule picks ' + got + ' on the shipped accent, theme.css picks '
      + (isWhite(shipped['--ac-tx']) ? 'white' : 'dark'));
  } else {
    pass(mode + ': the ink rule agrees with theme.css (' + got + ' on the accent)');
  }
}

// -- 4. an untouched token derives nothing ------------------------------------

const nothing = palette.derive(palette.empty(), 'dark');
if (Object.keys(nothing).length) {
  fail('an empty palette emitted ' + Object.keys(nothing).join(', '));
} else {
  pass('an empty palette emits nothing at all');
}

const oneChange = palette.derive({ dark: { ac: '#123456' } }, 'dark');
const strays = Object.keys(oneChange).filter((name) => !name.startsWith('--ac') && !name.startsWith('--term'));
if (strays.length) {
  fail('changing the accent also emitted ' + strays.join(', '));
} else {
  pass('changing one token emits only that token and what follows from it ('
    + Object.keys(oneChange).length + ' declarations)');
}

/*
 * The one value in the palette that is not six hex digits: a font stack, typed
 * by the user, landing in a stylesheet. What matters is not that it is
 * unchanged -- it is mangled on purpose -- but that it cannot become a second
 * declaration or a second rule.
 */
const fonts = palette.css({ font: 'Inter"; } body { display: none', fontMono: 'Cascadia Mono' });
const declarations = fonts.split('\n').filter((line) => line.trim().startsWith('--'));
const balanced = (fonts.match(/{/g) || []).length === (fonts.match(/}/g) || []).length;

if (!balanced) {
  fail('a font name opened or closed a rule:\n' + fonts);
} else if (declarations.length !== 2) {
  fail('a font name became ' + declarations.length + ' declarations:\n' + fonts);
} else if (declarations.some((line) => (line.match(/:/g) || []).length !== 1)) {
  fail('a font name carried a colon into its declaration:\n' + fonts);
} else {
  pass('a font name cannot escape the declaration it sits in');
}

console.log(failures ? '\n' + failures + ' failed' : '\nall good');
process.exit(failures ? 1 : 0);
