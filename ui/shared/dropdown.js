/*
 * The list half of a dropdown.
 *
 * `.c-select` in components.css draws the closed control -- the box, the border,
 * the chevron -- and for a while that was the whole of it, on the argument that
 * the popup is the platform's and the platform's popup follows the theme. It
 * does follow the theme. What it does not do is agree with itself: Chromium
 * draws a `<select>` popup and an `<input list>` popup with two different pieces
 * of code, and they share neither corner radius, row height, shadow, highlight
 * nor scrollbar. The scheme picker and the font box sit one above the other in
 * the settings page, they are the same size and the same shape while shut, and
 * opening them produced two visibly different objects.
 *
 * Neither popup can be styled. So this file draws the list instead.
 *
 * WHAT IT DOES NOT REPLACE
 *
 * The `<select>` and the `<input>` stay exactly where they are, visible, focusable
 * and holding the value. Every page that talks to them goes on reading `.value`,
 * assigning `.value`, and listening on `.onchange` -- settings.js, servers.js and
 * transfer.js were not touched to get this. The closed control the user sees is
 * still the real element, so the chosen text, the disabled state, the label
 * association and what a screen reader is told all come from the browser and not
 * from a div pretending.
 *
 * Only the moment between "clicked" and "chose" is ours. That keeps the surface
 * small: there is no shadow copy of the value to drift, and if this script
 * fails to load the controls still work -- they just open the platform's list.
 *
 * The `<datalist>` is left in the document as the data source; what is removed
 * is the `list=` attribute that makes Chromium draw its own suggestions. So the
 * page fills a datalist exactly as it did before and this reads it back.
 */
(function () {
  'use strict';

  /* The one popup on screen, and what it belongs to. Only ever one: opening a
   * second dropdown closes the first, the way menus behave everywhere. */
  var list = null;
  var host = null;     /* the `.c-select` */
  var control = null;  /* the <select> or <input> inside it */
  var rows = [];
  var active = -1;

  /*
   * Read at the moment the list opens, never cached.
   *
   * Both sources are filled by the page after the fact -- the scheme picker is
   * rebuilt on every `draw()`, and the font list arrives in a message from Rust
   * long after the page has loaded. Reading on open means there is nothing to
   * keep in step and no observer to forget to disconnect.
   */
  function options() {
    if (control.tagName === 'SELECT') {
      return Array.prototype.map.call(control.options, function (option) {
        return { value: option.value, label: option.textContent || option.value };
      });
    }
    var source = document.getElementById(control.getAttribute('data-list') || '');
    if (!source) return [];
    return Array.prototype.map.call(source.options, function (option) {
      return { value: option.value, label: option.label || option.textContent || option.value };
    });
  }

  /*
   * A `<select>` is a closed choice, so its list is never filtered -- what is in
   * the box is one of the rows, and typing in it is not a thing you can do. An
   * `<input>` is free text with suggestions, so what has been typed narrows
   * them. Typing a font stack with fallbacks in it will match nothing and show
   * nothing, which is correct: there is no suggestion for it, and the value is
   * still perfectly good.
   */
  function matching() {
    var all = options();
    if (control.tagName === 'SELECT') return all;
    var typed = control.value.trim().toLowerCase();
    if (!typed) return all;
    return all.filter(function (row) {
      return row.label.toLowerCase().indexOf(typed) !== -1;
    });
  }

  function close() {
    if (!list) return;
    list.remove();
    if (control) {
      control.removeAttribute('aria-expanded');
      control.removeAttribute('aria-activedescendant');
    }
    list = null;
    host = null;
    control = null;
    rows = [];
    active = -1;
  }

  /*
   * Fixed, and placed from the control's rect.
   *
   * Absolute inside `.c-select` would be less code and is wrong twice over: a
   * two-hundred-entry font list would extend the settings page's own scroll
   * height, so opening a dropdown would move the page's scrollbar, and any
   * ancestor that clips -- the preview pane on the transfer page does -- would
   * cut the list off. Fixed is measured against the viewport and clipped by
   * nothing; the cost is that it has to be closed when anything scrolls, which
   * is what a popup should do anyway.
   */
  function place() {
    var rect = host.getBoundingClientRect();
    var below = window.innerHeight - rect.bottom - 8;
    var above = rect.top - 8;
    /* Below unless there is meaningfully more room above -- a list that flips
     * upwards for the sake of ten more pixels is a list that appears somewhere
     * different each time. */
    var flip = below < 160 && above > below;

    list.style.left = Math.round(rect.left) + 'px';
    list.style.minWidth = Math.round(rect.width) + 'px';
    /* At least as wide as the control, and never past the edge of the window:
     * some of the font names are longer than the box that will hold them. */
    list.style.maxWidth = Math.round(window.innerWidth - rect.left - 8) + 'px';
    list.style.maxHeight = Math.round(Math.min(flip ? above : below, 288)) + 'px';
    if (flip) {
      list.style.top = 'auto';
      list.style.bottom = Math.round(window.innerHeight - rect.top + 4) + 'px';
    } else {
      list.style.bottom = 'auto';
      list.style.top = Math.round(rect.bottom + 4) + 'px';
    }
  }

  function commit(row) {
    var target = control;
    close();
    target.value = row.value;
    /* `input` first and then `change`, which is the order a person typing into
     * a field produces. settings.js listens on both -- `oninput` repaints the
     * terminals live and `onchange` writes the file -- so sending only one of
     * them would either not show or not save. */
    if (target.tagName !== 'SELECT') {
      target.dispatchEvent(new Event('input', { bubbles: true }));
    }
    target.dispatchEvent(new Event('change', { bubbles: true }));
    target.focus();
  }

  function highlight(next) {
    if (active >= 0 && rows[active]) rows[active].node.classList.remove('c-active');
    active = next;
    if (active < 0 || !rows[active]) {
      control.removeAttribute('aria-activedescendant');
      return;
    }
    var node = rows[active].node;
    node.classList.add('c-active');
    control.setAttribute('aria-activedescendant', node.id);
    /* `nearest`, so paging down the list moves it by a row rather than jumping
     * the highlighted item to the middle every time. */
    node.scrollIntoView({ block: 'nearest' });
  }

  function open(next) {
    if (host === next) return close();
    close();

    host = next;
    control = host.querySelector('select, input');
    if (!control || control.disabled) { host = null; control = null; return; }

    var items = matching();
    if (!items.length) { host = null; control = null; return; }

    list = document.createElement('div');
    list.className = 'c-select-list';
    list.setAttribute('role', 'listbox');
    list.id = 'c-select-list';

    var current = control.value;
    rows = items.map(function (row, index) {
      var node = document.createElement('div');
      node.className = 'c-select-option';
      node.id = 'c-select-option-' + index;
      node.setAttribute('role', 'option');
      node.textContent = row.label;
      /* Chosen is said in weight as well as in colour: a row that is only a
       * different hue is a row half the people reading it cannot pick out. */
      node.setAttribute('aria-selected', String(row.value === current));
      if (row.value === current) node.classList.add('c-current');
      /* `mousedown` and not `click`: the control has focus, and a click would
       * blur it on the way down and close the list before the button came up. */
      node.addEventListener('mousedown', function (event) {
        event.preventDefault();
        commit(row);
      });
      node.addEventListener('mousemove', function () {
        if (rows[active] !== rows[index]) highlight(index);
      });
      list.appendChild(node);
      return { node: node, value: row.value, label: row.label };
    });

    document.body.appendChild(list);
    control.setAttribute('aria-expanded', 'true');
    place();

    var start = rows.findIndex(function (row) { return row.value === current; });
    highlight(start);
  }

  /* -- wiring ------------------------------------------------------------- */

  function keys(event) {
    var shell = event.target.closest && event.target.closest('.c-select');
    if (!shell) return;

    if (event.key === 'Escape') {
      if (list) { event.stopPropagation(); close(); }
      return;
    }

    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!list) return open(shell);
      var step = event.key === 'ArrowDown' ? 1 : -1;
      highlight((active + step + rows.length) % rows.length);
      return;
    }

    if (list && (event.key === 'Home' || event.key === 'End')) {
      event.preventDefault();
      highlight(event.key === 'Home' ? 0 : rows.length - 1);
      return;
    }

    if (event.key === 'Enter') {
      if (list && active >= 0) { event.preventDefault(); commit(rows[active]); }
      return;
    }

    /*
     * Space opens a `<select>`, and must not do so in the text field beside it.
     * The native popup would open on the same key, so this is a swap rather
     * than an addition.
     */
    if (event.key === ' ' && event.target.tagName === 'SELECT' && !list) {
      event.preventDefault();
      open(shell);
    }
  }

  document.addEventListener('keydown', keys, true);

  /*
   * `mousedown` with the default prevented is what stops Chromium opening its
   * own popup for a `<select>`. There is no other way to suppress it, and it
   * has to happen before the browser acts, which is why this is on mousedown
   * and not on click.
   *
   * For an `<input>` the default must NOT be prevented -- it is where the caret
   * gets placed and where a selection is dragged. Its native list is already
   * gone, because `enhance` took the `list=` attribute off.
   */
  document.addEventListener('mousedown', function (event) {
    if (!event.target.closest) return close();
    /*
     * The rows are in `<body>`, not inside the `.c-select`, so without this the
     * capture listener would fire first, find no shell, and close the list --
     * every click on an option would land on nothing. The row has a handler of
     * its own; this one steps aside for it.
     */
    if (event.target.closest('.c-select-list')) return;
    var shell = event.target.closest('.c-select');
    if (!shell) return close();
    var field = shell.querySelector('select, input');
    if (!field || field.disabled) return;
    if (event.target === field && field.tagName === 'SELECT') {
      event.preventDefault();
      field.focus();
      open(shell);
    } else if (event.target === field && !list) {
      open(shell);
    }
  }, true);

  /* Typing narrows the list, and opens it if it was shut -- the suggestions are
   * most wanted at the moment somebody starts spelling a font name. */
  document.addEventListener('input', function (event) {
    var shell = event.target.closest && event.target.closest('.c-select');
    if (!shell || event.target.tagName === 'SELECT') return;
    close();
    /* `open` shows whatever now matches, and shows nothing at all when the text
     * matches nothing -- a font stack typed out in full is a legitimate value
     * with no suggestion behind it, and an empty popup hanging off it would be
     * the page insisting otherwise. */
    open(shell);
  });

  document.addEventListener('focusout', function (event) {
    /* Only when focus has left the control entirely: clicking a row moves focus
     * nowhere, because the row's mousedown is prevented. */
    if (list && event.target === control) setTimeout(function () {
      if (control && document.activeElement !== control) close();
    }, 0);
  });

  window.addEventListener('resize', close);
  /* Any scroll moves the control out from under a popup placed against the
   * viewport -- except a scroll of the popup itself, which is how a font list
   * is read. */
  window.addEventListener('scroll', function (event) {
    var node = event.target;
    if (node && node.closest && node.closest('.c-select-list')) return;
    close();
  }, true);

  /*
   * Take the `list=` attribute off, which is the whole of what makes Chromium
   * draw suggestions of its own. The `<datalist>` stays in the document and
   * stays the source; the id moves to `data-list` so this file can still find
   * it, and so that anything reading the markup can see where the options come
   * from.
   */
  function enhance(root) {
    (root || document).querySelectorAll('.c-select > input[list]').forEach(function (input) {
      input.setAttribute('data-list', input.getAttribute('list'));
      input.removeAttribute('list');
      input.setAttribute('role', 'combobox');
      input.setAttribute('aria-autocomplete', 'list');
      input.setAttribute('aria-controls', 'c-select-list');
    });
    (root || document).querySelectorAll('.c-select > select').forEach(function (select) {
      select.setAttribute('aria-controls', 'c-select-list');
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', function () { enhance(); });
  } else {
    enhance();
  }

  /* For a page that builds a `.c-select` after load. Nothing does today; it is
   * one line, and finding out the hard way costs an afternoon. */
  window.tshellDropdown = { enhance: enhance, close: close };
}());
