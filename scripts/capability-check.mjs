/*
 * Every window operation the shell calls, checked against what it is allowed
 * to call.
 *
 * Tauri v2 gates each window command behind a permission. `core:default` grants
 * the whole read half -- `isMaximized`, `isFullscreen`, `innerSize` -- and none
 * of the write half, so anything that *changes* the window has to be named in
 * `capabilities/default.json` one command at a time.
 *
 * Nothing says so at build time. A call the capability file does not cover
 * compiles, ships, and returns a rejected promise at the moment the user
 * presses the key -- and the shell catches its own promises, because a window
 * button that cannot minimise is not a reason to take the window down. So the
 * failure is a `console.warn` in a console nobody has open, under a key that
 * appears to do nothing.
 *
 * That is what happened to F11. `setFullscreen` was never in the capability
 * file, `isFullscreen` was (it comes with the defaults), so the call got one
 * step in and stopped -- including the line that hides the title bar, which sat
 * behind it. The shortcut matched, the action dispatched, and the window did
 * nothing at all.
 *
 *     node scripts/capability-check.mjs
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (path) => readFileSync(join(root, path), 'utf8');

let failures = 0;
const fail = (message) => { failures += 1; console.error('  FAIL  ' + message); };
const pass = (message) => console.log('  ok    ' + message);

const capability = JSON.parse(read('src-tauri/capabilities/default.json'));
const manifests = JSON.parse(read('src-tauri/gen/schemas/acl-manifests.json'));

/*
 * What the capability file grants, expanded.
 *
 * `core:default` is a set rather than a permission: it names one default per
 * core plugin, and `core:window:default` is the read half. Expanded here rather
 * than assumed, because assuming is how `allow-is-fullscreen` came to look like
 * it covered full screen.
 */
const granted = new Set();
for (const entry of capability.permissions) {
  if (entry === 'core:default') {
    for (const name of manifests.core.default_permission.permissions) {
      if (name !== 'core:window:default') continue;
      for (const allow of manifests['core:window'].default_permission.permissions) {
        granted.add('core:window:' + allow);
      }
    }
    continue;
  }
  granted.add(entry);
}

/*
 * What the shell calls on the window object. Event subscriptions (`onResized`)
 * and the handle itself (`getCurrentWindow`) are not commands and are not
 * gated; everything else is.
 */
const shell = read('ui/shell/shell.js');
const called = new Set(
  [...shell.matchAll(/\bappWindow\.([a-zA-Z]+)\s*\(/g)]
    .map((match) => match[1])
    .filter((name) => !name.startsWith('on') && name !== 'getCurrentWindow')
);

const kebab = (name) => name.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());

if (!called.size) fail('found no appWindow calls in shell.js -- has the shape changed?');

const ungranted = [];
for (const name of [...called].sort()) {
  const permission = 'core:window:allow-' + kebab(name);
  if (!granted.has(permission)) ungranted.push(name + ' needs ' + permission);
}

if (ungranted.length) {
  fail('the shell calls window operations it is not allowed to:\n        '
    + ungranted.join('\n        '));
} else {
  pass([...called].sort().join(', ') + ' -- every one of them granted');
}

/*
 * The other direction, and only a note: a permission granted for a call nobody
 * makes is a widened surface nobody meant to keep. Not a failure, because the
 * capability file is also where a human writes down what the window is allowed
 * to become.
 *
 * Dragging is the exception and is spelled out rather than special-cased by
 * name: it is asked for by `data-tauri-drag-region` in the markup, not by a
 * call, so it is used without ever appearing in the scan above. Left to the
 * rule it would be reported every single run, and a note that is always up is
 * furniture.
 */
const fromMarkup = read('ui/index.html').includes('data-tauri-drag-region')
  ? ['core:window:allow-start-dragging']
  : [];

const spare = [...granted]
  .filter((entry) => entry.startsWith('core:window:allow-'))
  .filter((entry) => capability.permissions.includes(entry))
  .filter((entry) => !fromMarkup.includes(entry))
  .filter((entry) => ![...called].some((name) => 'core:window:allow-' + kebab(name) === entry));
if (spare.length) console.log('  note  granted but not called: ' + spare.join(', '));

console.log(failures ? '\n' + failures + ' failed' : '\nall good');
process.exit(failures ? 1 : 0);
