/*
 * Keyboard shortcuts: what they are spelled as, what is allowed to be one, and
 * whether a given keystroke is one.
 *
 * Shared because two very different places need the same answers and must not
 * answer differently. `host.js` runs in every page and has to decide, on every
 * keydown, whether to take the key away from whatever was going to receive it.
 * The settings page has to say whether a combination the user just pressed may
 * be stored. Written twice, those two would drift, and the direction they drift
 * in is a shortcut that the panel accepts and nothing ever fires -- or worse,
 * one that fires in a terminal where the panel promised it would not.
 *
 * Rust holds the strings and checks their shape; it does not hold this rule.
 * The same split as the palette: `theme.rs` knows what a colour looks like,
 * `palette.js` knows what follows from one.
 *
 * -- spelling ---------------------------------------------------------------
 *
 * A binding is modifiers then a key, joined by `+`, and the key half is
 * `KeyboardEvent.code`: `Ctrl+Shift+KeyT`, `F11`, `Alt+Shift+Digit1`.
 *
 * `code` and not `key`, which is the physical key rather than the character it
 * produces. Alt changes that character on plenty of layouts -- on macOS
 * Option+Shift+P is not "P" at all -- so a binding stored as a character is a
 * binding that stops working when the user switches layout. terminal.js had
 * this right in the one hard-coded shortcut this file replaces, and the reason
 * is the same one.
 *
 * The cost is that the stored form is not the readable form: nobody wants to
 * see `Ctrl+Shift+KeyT` in a settings panel. `label()` is the translation, and
 * it only runs on the way to a screen.
 */
(function () {
  'use strict';

  /*
   * Every action a key can be bound to, and what it is bound to out of the box.
   *
   * The order is the order the settings page lists them. A fourth action is a
   * line here plus a case in the shell's dispatch, and nothing else -- the
   * panel, the storage, the conflict check and the matching all read this.
   */
  var ACTIONS = [
    { id: 'openTransfer', binding: 'Ctrl+Shift+KeyT' },
    { id: 'openAssistant', binding: 'Ctrl+Shift+KeyI' },
    { id: 'toggleFullscreen', binding: 'F11' }
  ];

  /*
   * One order, so one combination has one spelling. Without a fixed order
   * `Shift+Ctrl+KeyT` and `Ctrl+Shift+KeyT` are two strings for one keystroke,
   * and the conflict check would let both be stored.
   */
  var MODIFIERS = ['Ctrl', 'Alt', 'Meta', 'Shift'];

  /* A keystroke that is only a modifier is not a shortcut; it is half of one. */
  var MODIFIER_CODES = /^(Control|Shift|Alt|Meta|OS)(Left|Right)?$/;

  /* F1 through F24. Allowed to stand alone -- no shell reads them. */
  var FUNCTION_CODE = /^F([1-9]|1[0-9]|2[0-4])$/;

  var ids = {};
  ACTIONS.forEach(function (action) { ids[action.id] = action.binding; });

  function defaults() {
    var out = {};
    ACTIONS.forEach(function (action) { out[action.id] = action.binding; });
    return out;
  }

  /** `'Ctrl+Shift+KeyT'` to its parts, or null if it is not a binding at all. */
  function parse(text) {
    if (typeof text !== 'string' || !text) return null;
    var parts = text.split('+');
    var code = parts.pop();
    if (!code || !/^[A-Za-z][A-Za-z0-9]*$/.test(code)) return null;

    var out = { ctrl: false, alt: false, meta: false, shift: false, code: code };
    for (var i = 0; i < parts.length; i += 1) {
      switch (parts[i]) {
        case 'Ctrl': out.ctrl = true; break;
        case 'Alt': out.alt = true; break;
        case 'Meta': out.meta = true; break;
        case 'Shift': out.shift = true; break;
        default: return null;
      }
    }
    return out;
  }

  function spell(parts) {
    var out = [];
    MODIFIERS.forEach(function (name) {
      if (parts[name.toLowerCase()]) out.push(name);
    });
    out.push(parts.code);
    return out.join('+');
  }

  /**
   * The keystroke that just happened, in the stored spelling.
   *
   * Empty for a press that is only a modifier, which is what a user holding
   * Ctrl on the way to Ctrl+Shift+T produces three of.
   */
  function fromEvent(event) {
    if (!event || !event.code || MODIFIER_CODES.test(event.code)) return '';
    return spell({
      ctrl: !!event.ctrlKey,
      alt: !!event.altKey,
      meta: !!event.metaKey,
      shift: !!event.shiftKey,
      code: event.code
    });
  }

  /*
   * What a person reads. `KeyT` is `T`, `Digit1` is `1`, `Numpad3` is `Num 3`,
   * and an arrow is an arrow. Anything this does not know keeps its own name,
   * which is right for `F11`, `Home`, `Escape` and the rest.
   */
  var ARROWS = { ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→' };
  var NAMED = {
    Space: 'Space', Enter: 'Enter', Escape: 'Esc', Tab: 'Tab',
    Backquote: '`', Minus: '-', Equal: '=', Backslash: '\\',
    BracketLeft: '[', BracketRight: ']', Semicolon: ';', Quote: "'",
    Comma: ',', Period: '.', Slash: '/'
  };

  function keyLabel(code) {
    if (/^Key[A-Z]$/.test(code)) return code.slice(3);
    if (/^Digit[0-9]$/.test(code)) return code.slice(5);
    if (/^Numpad/.test(code)) return 'Num ' + code.slice(6);
    if (ARROWS[code]) return ARROWS[code];
    if (NAMED[code]) return NAMED[code];
    return code;
  }

  function label(binding) {
    var parts = parse(binding);
    if (!parts) return '';
    var out = [];
    MODIFIERS.forEach(function (name) {
      if (parts[name.toLowerCase()]) out.push(name);
    });
    out.push(keyLabel(parts.code));
    return out.join('+');
  }

  /*
   * Whether this may be a shortcut, and why not when it may not.
   *
   * The rule exists because of where these fire. A terminal has the keyboard
   * and every plain keystroke belongs to the shell on the far end -- and the
   * whole of `Ctrl` plus a letter belongs to readline: Ctrl+A start of line,
   * Ctrl+E end, Ctrl+K kill, Ctrl+W delete word, Ctrl+R search, Ctrl+C the
   * thing you press when you have made a mistake. Binding a window action to
   * any of them means the window eats a key the user was aiming at their shell,
   * and they cannot get it back without opening this panel again -- which they
   * will not think to do, because what they will believe is that the connection
   * broke.
   *
   * So: a function key stands alone, and everything else needs Shift on top of
   * a real modifier. Shift is what takes a combination out of the space any
   * shell uses; there is no `Ctrl+Shift+letter` in readline, in tmux's prefix
   * table, or in vi.
   *
   * Returns a reason key rather than a boolean so the panel can say which rule
   * was broken. An empty string is the answer that means yes.
   */
  function usable(binding) {
    var parts = parse(binding);
    if (!parts) return 'keysBadShape';
    if (MODIFIER_CODES.test(parts.code)) return 'keysModifierOnly';
    if (FUNCTION_CODE.test(parts.code)) return '';
    if (!parts.shift) return 'keysNeedsShift';
    if (!parts.ctrl && !parts.alt && !parts.meta) return 'keysNeedsModifier';
    return '';
  }

  /**
   * Whatever was stored, made safe to index into: known actions only, usable
   * bindings only, and no two actions on one keystroke.
   *
   * Only what differs from the default survives, the same rule the palette
   * follows: a binding nobody changed is absent from the file, so a later build
   * that picks a better default gets to hand it to everyone who never opened
   * the panel. Storing all three would freeze today's choice into every install
   * that ever saved once.
   */
  function normalize(file) {
    var out = {};
    if (!file || typeof file !== 'object') return out;
    var taken = {};
    ACTIONS.forEach(function (action) {
      var binding = file[action.id];
      if (typeof binding !== 'string') return;
      var parts = parse(binding);
      if (!parts) return;
      var clean = spell(parts);
      if (usable(clean)) return;
      if (clean === action.binding) return; // the default, written out; drop it
      if (taken[clean]) return;             // first action listed keeps it
      taken[clean] = true;
      out[action.id] = clean;
    });
    return out;
  }

  /*
   * The table that is actually matched against: the defaults, with the user's
   * edits laid over them.
   *
   * An edit that collides with a *default* wins, and the default it collided
   * with is dropped rather than left to fire as well. Rebinding "open the
   * assistant" to F11 has to take F11 away from full screen, or one keystroke
   * does two things and which one is a question about object key order.
   */
  function resolve(file) {
    var edits = normalize(file);
    var out = defaults();
    Object.keys(edits).forEach(function (id) {
      var binding = edits[id];
      Object.keys(out).forEach(function (other) {
        if (other !== id && out[other] === binding) delete out[other];
      });
      out[id] = binding;
    });
    return out;
  }

  /** Which action this keystroke is, if it is one. */
  function match(resolved, event) {
    var pressed = fromEvent(event);
    if (!pressed) return '';
    var hit = '';
    Object.keys(resolved || {}).forEach(function (id) {
      if (resolved[id] === pressed) hit = id;
    });
    return hit;
  }

  var api = {
    actions: ACTIONS,
    defaults: defaults,
    parse: parse,
    fromEvent: fromEvent,
    label: label,
    usable: usable,
    normalize: normalize,
    resolve: resolve,
    match: match
  };

  /*
   * A global, like the other shared scripts here: the pages have no module
   * loader and no build step. `globalThis` rather than `window` so the same
   * file can be required from node by a check script.
   */
  globalThis.tshellKeys = api;
  if (typeof module === 'object' && module.exports) module.exports = api;
})();
