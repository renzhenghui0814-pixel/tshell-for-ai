/*
 * What the shortcut table promises, checked.
 *
 * An action is spelled in four places that have to agree: `keys.js` lists it,
 * `shell.js` says what it does, and `settings.js` gives it a name in each of the
 * two languages. Nothing in the running program forces those to line up, and
 * every way they can come apart is quiet:
 *
 *   - an action with no case in the dispatch is a shortcut that fires, is
 *     delivered, and does nothing at all;
 *   - an action with no label is a row in the settings panel headed
 *     `openThing`;
 *   - a default binding the policy would refuse is a shortcut that ships bound
 *     and is dropped the first time the table is resolved;
 *   - two actions on one default is one of them silently losing its key.
 *
 * So this is the test for the parts that have no test runner. Run it after
 * touching any of the three:
 *
 *     node scripts/keys-check.mjs
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (path) => readFileSync(join(root, path), 'utf8');

await import(pathToFileURL(join(root, 'ui/shared/keys.js')).href);
const keys = globalThis.tshellKeys;

let failures = 0;
const fail = (message) => { failures += 1; console.error('  FAIL  ' + message); };
const pass = (message) => console.log('  ok    ' + message);

const ids = keys.actions.map((action) => action.id);

// -- 1. every action does something ------------------------------------------

const shell = read('ui/shell/shell.js');
const dispatch = (() => {
  const start = shell.indexOf('function runShortcut(');
  if (start === -1) throw new Error('shell.js has no runShortcut');
  return shell.slice(start, shell.indexOf('\n  }', start));
})();

const undispatched = ids.filter((id) => !dispatch.includes("'" + id + "'"));
if (undispatched.length) fail('no case in runShortcut for: ' + undispatched.join(', '));
else pass(ids.length + ' actions, every one of them dispatched in shell.js');

// -- 2. every action, and every refusal, has words ---------------------------

const settings = read('ui/settings/settings.js');
const twice = (key) => settings.split(key + ':').length - 1 === 2;

const nameless = ids
  .map((id) => 'keys' + id[0].toUpperCase() + id.slice(1))
  .filter((key) => !twice(key));
if (nameless.length) fail('no label in both languages for: ' + nameless.join(', '));
else pass(ids.length + ' actions named in both languages');

/*
 * Every reason `usable` can give, and the one the panel raises itself. Read out
 * of keys.js rather than listed here, so a new rule cannot be added with no
 * sentence to show for it.
 */
const reasons = [...new Set(
  [...read('ui/shared/keys.js').matchAll(/return '(keys[A-Z][A-Za-z]*)';/g)].map((match) => match[1])
), 'keysTaken'];

const unspoken = reasons.filter((key) => !twice(key));
if (unspoken.length) fail('no message in both languages for: ' + unspoken.join(', '));
else pass(reasons.length + ' refusals explained in both languages');

// -- 3. what ships is bindable ------------------------------------------------

const shipped = keys.defaults();
let bad = 0;
for (const id of ids) {
  const why = keys.usable(shipped[id]);
  if (why) { fail(id + ' ships bound to ' + shipped[id] + ', which the policy refuses (' + why + ')'); bad += 1; }
}

const seen = new Map();
for (const id of ids) {
  const binding = shipped[id];
  if (seen.has(binding)) { fail(binding + ' is the default for both ' + seen.get(binding) + ' and ' + id); bad += 1; }
  seen.set(binding, id);
}
if (!bad) pass(ids.length + ' shipped bindings are distinct and all pass the policy');

// -- 4. the policy actually protects the shell --------------------------------

/*
 * Not a restatement of the rule -- a list of the keystrokes it exists to keep
 * out. Every one of these is something a person types at a shell, and every one
 * of them would be swallowed by this window if the policy let it be bound.
 */
const MUST_REFUSE = [
  'Ctrl+KeyA', 'Ctrl+KeyE', 'Ctrl+KeyK', 'Ctrl+KeyW', 'Ctrl+KeyR', 'Ctrl+KeyC',
  'Ctrl+KeyD', 'Ctrl+KeyU', 'Ctrl+KeyL', 'Ctrl+KeyZ',
  'KeyA', 'Digit1', 'Space', 'Enter', 'Tab', 'Shift+KeyA', 'Alt+KeyB', 'Alt+KeyF'
];
const leaked = MUST_REFUSE.filter((binding) => !keys.usable(binding));
if (leaked.length) fail('the policy would allow: ' + leaked.join(', '));
else pass(MUST_REFUSE.length + ' keystrokes that belong to the shell are refused');

// -- 5. resolving is total ----------------------------------------------------

const resolved = keys.resolve({});
const missing = ids.filter((id) => !resolved[id]);
if (missing.length) fail('resolve() left these unbound with no edits: ' + missing.join(', '));
else pass('an empty file resolves to every action being bound');

const stolen = keys.resolve({ [ids[0]]: shipped[ids[ids.length - 1]] });
if (stolen[ids[ids.length - 1]]) fail('an edit did not take the binding from the action that had it');
else pass('rebinding onto a default takes the key away from it');

console.log(failures ? '\n' + failures + ' failed' : '\nall good');
process.exit(failures ? 1 : 0);
