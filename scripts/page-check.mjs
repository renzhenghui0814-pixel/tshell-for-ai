/*
 * Every shared global a page uses is provided by a script that page loads.
 *
 * There is no build step and no module system: `ui/shared/*.js` files publish
 * themselves onto `window`, and a page gets them by listing a `<script>` tag.
 * Nothing checks that the list matches what the page reaches for. Miss one and
 * the page loads, renders whatever runs before the first call, and then stops --
 * with the failure in a console no one is looking at, on the far side of a
 * WebView that is deliberately hard to attach a debugger to.
 *
 * That is exactly how the editor pane shipped with `highlight.css` and no
 * `highlight.js`: it drew its title bar, threw on the first line it tried to
 * colour, and showed an empty pane with the right filename on the tab.
 *
 *   node scripts/page-check.mjs
 */
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { join, dirname, resolve, relative } from 'node:path';

const root = resolve(new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'));
const ui = join(root, 'ui');

let failures = 0;
const fail = (message) => { failures += 1; console.log('  FAIL  ' + message); };
const pass = (message) => console.log('  ok    ' + message);

/* -- who provides what ----------------------------------------------------- */

/*
 * Read the assignments rather than keeping a list here. A table would be a
 * second place to remember something, and the whole point of this script is
 * that the first place is already not being remembered.
 */
const provider = new Map();
for (const name of readdirSync(join(ui, 'shared')).filter((f) => f.endsWith('.js'))) {
  const source = readFileSync(join(ui, 'shared', name), 'utf8');
  for (const [, global] of source.matchAll(/\bwindow\.(tshell[A-Za-z0-9]*)\s*=/g)) {
    provider.set(global, 'shared/' + name);
  }
  // host.js supplies the page contract under a name that is not its own.
  if (/\bwindow\.acquireVsCodeApi\s*=/.test(source)) {
    provider.set('acquireVsCodeApi', 'shared/' + name);
  }
}

/*
 * Globals a page is allowed to reach for without loading a provider.
 *
 * `tshellShortcutsPaused` is a flag host.js declares and a page sets; the pages
 * that set it load host.js anyway, and requiring a provider for a boolean would
 * be noise. The bootstrap names are assigned by host.js and read by the pages,
 * which is the same relationship.
 */
const contract = new Set([
  'tshellShortcutsPaused', 'tshellBootstrap', 'tshellPaneId',
  'tshellTransferBuild', 'tshellChatBuild'
]);

/* -- what each page uses --------------------------------------------------- */

const pages = [];
for (const entry of readdirSync(ui, { withFileTypes: true })) {
  if (!entry.isDirectory() || entry.name === 'vendor' || entry.name === 'shared') continue;
  for (const file of readdirSync(join(ui, entry.name))) {
    if (file.endsWith('.html') && !file.startsWith('_')) pages.push(join(ui, entry.name, file));
  }
}
if (existsSync(join(ui, 'index.html'))) pages.push(join(ui, 'index.html'));

for (const page of pages) {
  const html = readFileSync(page, 'utf8');
  const here = dirname(page);

  // The scripts this page loads, as paths under ui/.
  const loaded = new Set();
  for (const [, src] of html.matchAll(/<script[^>]+src="([^"]+)"/g)) {
    loaded.add(relative(ui, resolve(here, src)).split('\\').join('/'));
  }

  // Everything it runs: its own inline script tags and every file it loads
  // from its own directory. Shared files are not scanned -- what they use is
  // their own business and they are loaded in dependency order by hand.
  let code = html;
  for (const src of loaded) {
    if (src.startsWith('shared/') || src.startsWith('vendor/')) continue;
    const path = join(ui, src);
    if (existsSync(path)) code += '\n' + readFileSync(path, 'utf8');
  }

  const wanted = new Set();
  for (const [, global] of code.matchAll(/\bwindow\.(tshell[A-Za-z0-9]*)\b/g)) wanted.add(global);
  for (const [] of code.matchAll(/\bacquireVsCodeApi\s*\(/g)) wanted.add('acquireVsCodeApi');

  const label = relative(ui, page).split('\\').join('/');
  const missing = [];
  for (const global of wanted) {
    if (contract.has(global)) continue;
    const from = provider.get(global);
    if (!from) continue;           // Something the page declares itself.
    if (!loaded.has(from)) missing.push(global + ' (needs ' + from + ')');
  }

  if (missing.length) fail(label + ' uses ' + missing.sort().join(', '));
  else pass(label + ' loads everything it reaches for');
}

console.log(failures ? '\n' + failures + ' page(s) missing a script' : '\nall good');
process.exit(failures ? 1 : 0);
