/*
 * The shell: the only frame in this window that talks to Rust.
 *
 * Everything the user actually looks at lives in a page under ui/, unchanged
 * from when VS Code hosted it. Those pages post to their parent; this file
 * decides what that means and, in the finished product, forwards it over IPC.
 *
 * The forwarding is real for the server panel and for terminals. Sessions are
 * the one thing here that does not travel as a command result: each has a
 * `Channel` carrying raw bytes, because JSON-encoding a shell's output is enough
 * on its own to make the window stutter. File transfer and the assistant are
 * still stubbed, and say so in the console.
 */
(function () {
  'use strict';

  var tauri = window.__TAURI__ && window.__TAURI__.core;
  var invoke = tauri
    ? tauri.invoke
    : function (command) {
        return Promise.reject(new Error('not running under Tauri: ' + command));
      };

  var strings = {};
  var language = 'zh-CN';
  var theme = 'dark';

  /*
   * The terminals' appearance.
   *
   * `schemes.js` holds the three that ship and merges the user's onto them;
   * `tshell.schemes.json` holds the user's and is Rust's. This is the only
   * frame that knows both, which is why the merge happens here and the pages
   * are handed a finished appearance rather than the parts of one.
   *
   * `schemesError` is set when the file is on disk but would not parse. It is
   * not a reason to open without a terminal: the built-in schemes are enough to
   * paint with, and what it does buy is that nothing is written back over a
   * file we could not read -- the settings page shows the reason and stops
   * offering to save, exactly as the server panel does with a broken config.
   */
  var schemesError = '';
  var fontList = null;

  /*
   * Panes are flat and columns are ordered. A pane knows which column it belongs
   * to by id, never by where it sits in the DOM -- every frame in the window is a
   * child of the same grid from the moment it opens until it closes.
   */
  var panes = new Map();
  var columns = [];
  var splitters = [];
  var paneSeq = 0;

  /*
   * A tag that is different on every load of this document, mixed into every
   * pane id.
   *
   * `paneSeq` counts from zero in a fresh document, so without this the first
   * terminal after a reload is called `terminal-1` again -- and Rust's session
   * table does not reload with us. Two different tabs answering to one name is
   * how a live session ends up being torn down by the ghost of a dead one; see
   * releaseSessions() for the full sequence.
   *
   * releaseSessions() should mean the table is empty by the time we come back,
   * so this is the second line rather than the first. It is worth having: a
   * reload is not only ever a language switch -- a crash, a devtools refresh or
   * some future setting can all bring one about, and none of those get to run
   * our teardown first.
   */
  var paneEpoch = Date.now().toString(36) + Math.random().toString(36).slice(2, 5);
  var columnSeq = 0;
  var focusedColumn = null;

  /* No column ends up narrower than this, by any route that can make one. */
  var MIN_COLUMN = 240;

  /*
   * Restored before anything is drawn, so a light-theme window does not open
   * dark and then correct itself. Real settings move into the config file with
   * stage 5; until then this is the one preference worth not forgetting.
   */
  try {
    var saved = localStorage.getItem('tshell:theme');
    if (saved === 'light' || saved === 'dark') theme = saved;
  } catch (error) {
    // No store, no memory. The defaults stand.
  }
  if (theme === 'light') document.documentElement.setAttribute('data-theme', 'light');

  var panesEl = document.getElementById('panes');
  var sidebar = document.getElementById('sidebar');
  var actionAi = document.getElementById('act-ai');
  var actionTransfer = document.getElementById('act-transfer');

  /*
   * Switched off means gone, here as it was under VS Code -- the assistant's
   * button leaves the bar rather than sitting there refusing. Read from the
   * config on load and again whenever the settings page changes it.
   */
  var aiEnabled = true;

  /*
   * Re-reads the panel state after something outside the sidebar changed it.
   *
   * Only the assistant's switch does, so far. It has to reach three places that
   * are not the settings page -- the activity bar's button, the terminal pages'
   * own shortcut, and the server panel's row menu -- and none of them is a chat
   * pane, so `broadcastAiState` cannot carry it.
   */
  function refreshState() {
    return invoke('load_state').then(function (loaded) {
      panel = loaded;
      aiEnabled = loaded.aiEnabled !== false;
      actionAi.hidden = !aiEnabled;
      paintSidebar();
      panes.forEach(function (pane) {
        if (pane.kind === 'terminal') send(pane.id, { type: 'aiEnabled', enabled: aiEnabled });
      });
    }).catch(function (error) {
      console.warn('[shell] the settings changed but the panel would not reload:', error);
    });
  }

  function t(key, arg) {
    var value = (strings[language] && strings[language][key]) || key;
    return arg === undefined ? value : value.replace('{0}', arg);
  }

  // ---------------------------------------------------------------- panes ---

  /*
   * Bootstrap travels in the URL fragment, which is what lets the pages keep
   * reading it synchronously the way they did under VS Code. See shared/host.js.
   */
  function frameUrl(page, paneId, bootstrap) {
    var frame = { paneId: paneId, bootstrap: bootstrap, theme: theme };
    return page + '#' + encodeURIComponent(JSON.stringify(frame));
  }

  // --------------------------------------------------------------- columns ---

  function columnById(id) {
    for (var i = 0; i < columns.length; i += 1) {
      if (columns[i].id === id) return columns[i];
    }
    return null;
  }

  function columnOf(paneId) {
    var pane = panes.get(paneId);
    return pane ? columnById(pane.columnId) : null;
  }

  /*
   * A column is a tab bar and an order. It owns no frames -- panes stay children
   * of the grid for their whole lives and only ever have `grid-column` rewritten
   * beneath them, because moving an <iframe> in the DOM discards its browsing
   * context and would reconnect the session every time a tab was dragged.
   */
  function makeColumn(index) {
    var column = {
      id: 'col-' + (columnSeq += 1),
      weight: 1,
      order: [],
      active: null,
      tabbar: document.createElement('nav')
    };
    column.tabbar.className = 'tabbar';
    column.tabbar.addEventListener('contextmenu', function (event) {
      event.preventDefault();
    });

    /*
     * A wheel over the strip walks it sideways.
     *
     * The strip only scrolls horizontally, and a mouse only has a vertical
     * wheel. Chromium redirects one to the other on its own in some
     * circumstances and not in others, which is worse than either -- so it is
     * done here, from whichever axis the wheel reported, and the browser is
     * told not to do it a second time.
     *
     * `passive: false` because `preventDefault` in a wheel listener is ignored
     * without it: wheel listeners default to passive so that a page cannot
     * block scrolling by accident.
     */
    column.tabbar.addEventListener('wheel', function (event) {
      var strip = column.tabbar;
      if (strip.scrollWidth <= strip.clientWidth) return;
      var by = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
      if (!by) return;
      event.preventDefault();
      strip.scrollLeft += by;
    }, { passive: false });
    panesEl.appendChild(column.tabbar);
    columns.splice(index, 0, column);
    // Adding a column is a deliberate act on the layout, so the shares are
    // dealt again rather than the newcomer taking whatever was left over.
    columns.forEach(function (each) { each.weight = 1; });
    return column;
  }

  function dropColumn(column) {
    var index = columns.indexOf(column);
    if (index < 0) return;
    column.tabbar.remove();
    columns.splice(index, 1);
    if (focusedColumn === column.id) {
      var next = columns[Math.min(index, columns.length - 1)];
      focusedColumn = next ? next.id : null;
    }
    rescale();
  }

  /*
   * Scale the weights so they add up to the number of columns.
   *
   * Proportions are kept; the total is not left to chance, and it cannot be.
   * A set of `fr` factors summing to less than 1 does not fill its container --
   * the grid hands each track that fraction of the free space and leaves the
   * remainder blank. Dragging a splitter keeps a pair's total constant, so two
   * columns can sit at 0.75 and 1.25; drop the second and the survivor is left
   * alone at 0.75fr, taking three quarters of the window with a quarter of it
   * empty. That is a tab bar that stops short of the right edge.
   */
  function rescale() {
    if (!columns.length) return;
    var total = 0;
    columns.forEach(function (column) { total += column.weight; });
    if (!(total > 0)) {
      columns.forEach(function (column) { column.weight = 1; });
      return;
    }
    var scale = columns.length / total;
    columns.forEach(function (column) { column.weight *= scale; });
  }

  /** Tracks are 1-based and every second one is a splitter. */
  function trackOf(index) {
    return String(index * 2 + 1);
  }

  function paintTracks() {
    if (!columns.length) {
      panesEl.style.gridTemplateColumns = 'minmax(0, 1fr)';
      return;
    }
    var tracks = [];
    columns.forEach(function (column, i) {
      if (i) tracks.push('4px');
      /*
       * minmax(0, ...) rather than a bare fr. An iframe's automatic minimum size
       * is 300px wide, and a track that honours it can never be dragged below
       * it -- the column would simply refuse to shrink.
       */
      tracks.push('minmax(0, ' + column.weight.toFixed(4) + 'fr)');
    });
    panesEl.style.gridTemplateColumns = tracks.join(' ');
  }

  /** Splitters go between columns, so there is always one fewer of them. */
  function paintSplitters() {
    var wanted = Math.max(columns.length - 1, 0);
    while (splitters.length > wanted) splitters.pop().remove();
    while (splitters.length < wanted) {
      var bar = document.createElement('div');
      bar.className = 'col-splitter';
      bar.setAttribute('role', 'separator');
      bar.setAttribute('aria-orientation', 'vertical');
      bar.addEventListener('pointerdown', beginColumnResize);
      panesEl.appendChild(bar);
      splitters.push(bar);
    }
    splitters.forEach(function (each, i) {
      each.style.gridColumn = String(i * 2 + 2);
      each.style.gridRow = '1 / 3';
    });
  }

  function paintLayout() {
    paintSplitters();
    paintTracks();
    columns.forEach(function (column, i) {
      var track = trackOf(i);
      column.tabbar.style.gridColumn = track;
      column.tabbar.style.gridRow = '1';
      column.order.forEach(function (paneId) {
        var pane = panes.get(paneId);
        pane.iframe.style.gridColumn = track;
        pane.iframe.style.gridRow = '2';
      });
    });
    paintPanes();
  }

  /** Which tab shows in each column, and which column is in front. */
  function paintPanes() {
    columns.forEach(function (column) {
      column.tabbar.classList.toggle('focused', column.id === focusedColumn);
      column.order.forEach(function (paneId) {
        var pane = panes.get(paneId);
        var on = column.active === paneId;
        pane.iframe.hidden = !on;
        pane.tab.classList.toggle('active', on);
      });
    });
    syncActions();
  }

  // ----------------------------------------------------------------- panes ---

  function openPane(kind, options) {
    // `options.paneId` is for a tab whose id had to exist before the tab did:
    // the chat panel asks Rust to open a session under it, and the answer is
    // what its bootstrap is built from.
    var paneId = options.paneId || kind + '-' + paneEpoch + '-' + (paneSeq += 1);
    // `options.column` is for a tab whose place is decided by another tab
    // rather than by wherever the focus happens to be: a copied session, and
    // the assistant and transfer pages, which open beside the terminal they
    // were opened from.
    var column = options.column || columnById(focusedColumn) || makeColumn(columns.length);
    var pane = {
      id: paneId,
      kind: kind,
      columnId: column.id,
      title: options.title,
      server: options.server || null,
      groupId: options.groupId || null,
      // The terminal a chat pane borrows. Null on every other kind.
      terminal: options.terminal || null,
      // A fake terminal needs somewhere to keep the line being typed.
      line: ''
    };

    var iframe = document.createElement('iframe');
    iframe.src = frameUrl(options.page, paneId, options.bootstrap);
    iframe.hidden = true;
    pane.iframe = iframe;
    panesEl.appendChild(iframe);

    pane.tab = makeTab(pane);
    column.tabbar.appendChild(pane.tab);
    column.order.push(paneId);

    panes.set(paneId, pane);
    paintLayout();
    activate(paneId);
    return pane;
  }

  /*
   * A tab's glyph says what kind of thing it is, which the title cannot: two
   * tabs on the same server are called the same thing whether one is a shell
   * and the other a file panel. It also survives the title being truncated,
   * which at 210px of tab and a long hostname is most of the time.
   */
  var TAB_GLYPHS = {
    terminal: 'terminal',
    transfer: 'transfer',
    chat: 'ai',
    settings: 'gear'
  };

  function makeTab(pane) {
    var tab = document.createElement('div');
    tab.className = 'tab';

    var glyph = TAB_GLYPHS[pane.kind];
    if (glyph) tab.appendChild(window.tshellIcon(glyph, 'tab-glyph'));

    var title = document.createElement('span');
    title.className = 'tab-title';
    title.textContent = pane.title;
    tab.appendChild(title);

    var close = document.createElement('button');
    close.className = 'tab-close';
    close.type = 'button';
    close.textContent = '\u00d7';
    close.onclick = function (event) {
      event.stopPropagation();
      closePane(pane.id);
    };
    tab.appendChild(close);

    /*
     * The left button activates and may go on to drag; the right opens the menu
     * without activating, because a menu is a question about a tab and should
     * not answer it by bringing the tab forward.
     *
     * Activation happens on the press rather than the click, since a press that
     * finishes somewhere else is a drag and its click would arrive too late to
     * be worth anything.
     */
    tab.addEventListener('pointerdown', function (event) {
      if (event.target.closest('.tab-close')) return;
      if (event.button !== 0) return;
      activate(pane.id);
      beginTabDrag(pane, event);
    });

    tab.addEventListener('contextmenu', function (event) {
      event.preventDefault();
      event.stopPropagation();
      openTabMenu(pane, event.clientX, event.clientY);
    });

    return tab;
  }

  function activate(paneId) {
    var pane = panes.get(paneId);
    var column = pane ? columnById(pane.columnId) : null;
    if (!column) return;
    column.active = paneId;
    focusedColumn = column.id;
    paintPanes();
    revealTab(pane);
  }

  /*
   * Bring a tab into the strip if it has been scrolled out of it.
   *
   * With enough tabs open the strip is wider than the column, and the tab you
   * are working in is regularly not one of the ones on screen -- most of all
   * when the pane was reached by clicking into it rather than by clicking its
   * tab, which is the usual way into a terminal.
   *
   * `inline: 'nearest'` scrolls by the least that will do it and leaves the
   * strip alone when the tab is already visible; `block: 'nearest'` is what
   * stops it scrolling anything vertically on the way -- the panes are frames
   * and none of them should move because a tab was selected.
   */
  function revealTab(pane) {
    if (!pane || !pane.tab || !pane.tab.isConnected) return;
    pane.tab.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }

  /*
   * A click inside a pane is a click this document never hears -- the frame
   * keeps it. shared/host.js reports it back, so that the focus follows where
   * the user is actually working and not only where they last clicked a tab.
   */
  function focusPane(paneId) {
    var pane = panes.get(paneId);
    if (!pane) return;
    /*
     * The tab is brought into view even when the focused column has not
     * changed, which is the common case: clicking into the terminal you were
     * already in should still show you which tab you are in, and after enough
     * tabs have been opened that one is often off the end of the strip.
     */
    revealTab(pane);
    if (pane.columnId === focusedColumn) return;
    focusedColumn = pane.columnId;
    paintPanes();
  }

  /** Whether this pane is the one its column is currently showing. */
  function showing(pane) {
    var column = columnById(pane.columnId);
    return !!column && column.active === pane.id;
  }

  /** The terminal the title bar's two centre buttons act on, if any. */
  function frontTerminal() {
    var column = columnById(focusedColumn);
    var pane = column && column.active ? panes.get(column.active) : null;
    return pane && pane.kind === 'terminal' && pane.server ? pane : null;
  }

  function syncActions() {
    var on = !!frontTerminal();
    actionAi.disabled = !on;
    actionTransfer.disabled = !on;
    paintStatus();
  }

  // ---------------------------------------------------------- status bar ---

  /*
   * One line under the window saying what the tab in front is talking to.
   *
   * Every field is read off the `pane` record this file already keeps -- the
   * connection flags, `pane.server`, the size last reported by the page. Rust is
   * not asked for anything and no page had to be changed to supply it, which is
   * also why the bar lives out here rather than inside terminal.html: that page
   * fits xterm to its container and bails out of a fit when the container
   * measures zero, and hanging a 26px strip inside that box would have turned a
   * status line into a question about the fit addon.
   */
  var statusBar = document.getElementById('statusbar');
  var statusState = document.getElementById('st-state');
  var statusDot = document.getElementById('st-dot');
  var statusStateText = document.getElementById('st-state-text');
  var statusWho = document.getElementById('st-who');
  var statusEncoding = document.getElementById('st-encoding');
  var statusSize = document.getElementById('st-size');

  function frontPane() {
    var column = columnById(focusedColumn);
    if (!column || !column.active) return null;
    return panes.get(column.active) || null;
  }

  /*
   * `null` where this file does not actually track a connection, and the field
   * is then hidden rather than guessed at. A chat tab has no connection of its
   * own -- what it has is a run, which is a different thing and belongs to the
   * page that owns it -- and settings has nothing at all. Showing a confident
   * green dot for either would be the bar inventing news.
   */
  function connectionState(pane) {
    if (pane.kind === 'terminal') {
      if (pane.connected) return 'ok';
      if (pane.connecting) return 'busy';
      return 'off';
    }
    if (pane.kind === 'transfer') return pane.remoteReady ? 'ok' : 'busy';
    return null;
  }

  var STATE_WORD = { ok: 'stConnected', busy: 'stConnecting', off: 'stOffline' };
  var STATE_TONE = { ok: 'is-ok', busy: '', off: 'is-err' };

  function encodingLabel(encoding) {
    return encoding === 'gb18030' ? 'GB18030' : 'UTF-8';
  }

  function paintStatus() {
    // `var` hoists the name but not the lookup, and syncActions is reachable
    // from the layout pass. If anything ever paints before this file has run
    // its own top level, do nothing rather than throw inside the layout.
    if (!statusBar) return;
    var pane = frontPane();
    if (!pane) {
      statusBar.hidden = true;
      return;
    }
    statusBar.hidden = false;

    var state = connectionState(pane);
    statusState.hidden = !state;
    if (state) {
      statusDot.className = 'c-dot' + (state === 'ok' ? ' c-ok' : state === 'busy' ? ' c-busy' : '');
      statusStateText.textContent = c(STATE_WORD[state]);
      statusState.className = 'status-item status-state ' + STATE_TONE[state];
    }

    statusWho.hidden = !pane.server;
    if (pane.server) statusWho.textContent = who(pane);

    statusEncoding.hidden = !pane.server;
    if (pane.server) statusEncoding.textContent = encodingLabel(pane.server.encoding);

    // The size is the terminal's grid, so it means nothing on the other kinds.
    var sized = pane.kind === 'terminal' && pane.cols && pane.rows;
    statusSize.hidden = !sized;
    if (sized) statusSize.textContent = pane.cols + ' × ' + pane.rows;
  }

  /*
   * Hand every session back to Rust, and resolve once it has taken them.
   *
   * For the one thing in the window that rebuilds the whole document: changing
   * the language, which reloads because every page reads its labels out of the
   * bootstrap it loaded with.
   *
   * A reload destroys this document and everything in it, but it does not touch
   * Rust -- the SSH connections, their ptys and their pump tasks all keep
   * running, now with nothing on this side listening. Every language switch used
   * to leak the lot.
   *
   * Worse than the leak was what the leak did to the *next* connection. Sessions
   * are keyed by pane id, `paneSeq` restarts at zero on a fresh document, and so
   * the first terminal opened after a switch asked for a name a leaked session
   * still held. Rust's `open` inserts over whatever is there, which dropped the
   * stale session's sender; the stale pump read that as "the tab is gone", tore
   * itself down, and on its way out removed the registry entry -- by then the
   * *new* tab's. The new terminal came up connected, with nothing able to reach
   * its shell, and printed "connection closed" a moment later.
   *
   * Failures are swallowed: the user asked for the language to change, and a
   * session that will not close politely is not a reason to strand them on a
   * window that never reloads. Rust drops it when the process does.
   */
  function releaseSessions() {
    var closing = [];
    panes.forEach(function (pane) {
      if (pane.kind === 'terminal') closing.push(invoke('terminal_close', { pane: pane.id }));
      if (pane.kind === 'transfer') {
        clearInterval(pane.watch);
        closing.push(invoke('transfer_close', { pane: pane.id }));
      }
      if (pane.kind === 'chat') closing.push(invoke('ai_close', { pane: pane.id }));
    });
    return Promise.all(closing.map(function (done) {
      return done.catch(function () {});
    }));
  }

  function closePane(paneId) {
    var pane = panes.get(paneId);
    var column = pane ? columnById(pane.columnId) : null;
    if (!column) return;

    // The connection belongs to the tab, not to the window. Closing one without
    // the other leaves a shell running on the far side with nobody reading it.
    if (pane.kind === 'terminal') invoke('terminal_close', { pane: paneId });
    if (pane.kind === 'transfer') {
      clearInterval(pane.watch);
      invoke('transfer_close', { pane: paneId });
    }
    if (pane.kind === 'chat') invoke('ai_close', { pane: paneId });

    pane.iframe.remove();
    pane.tab.remove();
    panes.delete(paneId);

    var at = column.order.indexOf(paneId);
    column.order.splice(at, 1);
    if (column.active === paneId) {
      // The neighbour on the right, or the one on the left if there was none.
      column.active = column.order[Math.min(at, column.order.length - 1)] || null;
    }
    if (!column.order.length) dropColumn(column);
    paintLayout();
  }

  /*
   * Every tab in the window but this one, across every column.
   *
   * The ids are taken before anything is closed. `closePane` deletes from
   * `panes` and can drop a whole column with the last tab in it, so iterating
   * the live map would be walking a collection while it is being emptied.
   */
  function closeOthers(keepId) {
    var doomed = [];
    panes.forEach(function (pane) { if (pane.id !== keepId) doomed.push(pane.id); });
    doomed.forEach(closePane);
  }

  /*
   * Every rearrangement of tabs goes through here: the drag, both split
   * commands, and nothing else. `index` counts positions in the target column
   * with the moving tab already taken out, which is what callers measure.
   */
  function placePane(paneId, column, index) {
    var pane = panes.get(paneId);
    var from = pane ? columnById(pane.columnId) : null;
    if (!from) return;

    var at = from.order.indexOf(paneId);
    from.order.splice(at, 1);
    index = Math.max(0, Math.min(index, column.order.length));
    column.order.splice(index, 0, paneId);
    pane.columnId = column.id;

    // The tab is a div, so re-parenting it costs nothing. The frame stays put.
    var after = column.order[index + 1];
    column.tabbar.insertBefore(pane.tab, after ? panes.get(after).tab : null);

    if (from !== column) {
      if (from.active === paneId) {
        from.active = from.order[Math.min(at, from.order.length - 1)] || null;
      }
      if (!from.order.length) dropColumn(from);
    }
    paintLayout();
    activate(paneId);
  }

  /*
   * Splitting moves a tab; it does not copy one. A terminal tab is a single SSH
   * channel and there is no second one to give the new column.
   *
   * Which is also why the last tab in a column cannot be split off: its column
   * would empty and be swept away as it left, putting it back exactly where it
   * started. The menu greys the entries out rather than doing nothing visible.
   */
  function roomForColumn() {
    return panesEl.clientWidth / (columns.length + 1) >= MIN_COLUMN;
  }

  function canSplit(pane) {
    var column = columnById(pane.columnId);
    return !!column && column.order.length > 1 && roomForColumn();
  }

  /*
   * The column immediately right of a pane's, made if it is not there yet.
   *
   * The assistant and the transfer page are both opened *from* a terminal and
   * are meant to be read beside it -- the whole point of either is having the
   * shell and the thing you are doing to it on screen at once. Landing them as
   * another tab on top of that terminal hides the one window they exist to sit
   * next to.
   *
   * An existing column to the right is used rather than a new one inserted in
   * front of it: pressing the button twice should not keep pushing the user's
   * layout sideways, and "the next column along" is what they asked for either
   * way.
   *
   * With no room for another column they fall back to the terminal's own. A
   * 200px column is not somewhere to read a diff, and `roomForColumn` is what
   * decides that rather than a guess made here.
   */
  function columnRightOf(pane) {
    var column = pane && columnById(pane.columnId);
    if (!column) return null;
    var index = columns.indexOf(column) + 1;
    if (index < columns.length) return columns[index];
    return roomForColumn() ? makeColumn(index) : column;
  }

  function splitPane(paneId, side) {
    var pane = panes.get(paneId);
    if (!pane || !canSplit(pane)) return;
    var index = columns.indexOf(columnById(pane.columnId)) + (side === 'right' ? 1 : 0);
    placePane(paneId, makeColumn(index), 0);
  }

  // -------------------------------------------------------------- dragging ---

  /*
   * Pointer events, not HTML5 drag and drop.
   *
   * Native dragging would have to share the window with Tauri's own file-drop
   * handling, and every pane is a frame that takes drag events for itself the
   * moment the cursor crosses it. A captured pointer answers to neither, and the
   * sidebar splitter below already works this way.
   */
  var drag = null;

  var dropMark = document.createElement('div');
  dropMark.className = 'drop-mark';
  dropMark.hidden = true;
  panesEl.appendChild(dropMark);

  function beginTabDrag(pane, event) {
    drag = {
      paneId: pane.id,
      tab: pane.tab,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      moving: false,
      ghost: null
    };
    pane.tab.setPointerCapture(event.pointerId);
    pane.tab.addEventListener('pointermove', onDragMove);
    pane.tab.addEventListener('pointerup', onDragEnd);
    pane.tab.addEventListener('pointercancel', endDrag);
  }

  /*
   * Four pixels of slack before a press becomes a drag, so that an ordinary
   * click on a tab stays an ordinary click.
   */
  function onDragMove(event) {
    if (!drag) return;
    if (!drag.moving) {
      var far = Math.abs(event.clientX - drag.startX) > 4 ||
                Math.abs(event.clientY - drag.startY) > 4;
      if (!far) return;
      liftTab();
    }
    drag.ghost.style.left = (event.clientX + 12) + 'px';
    drag.ghost.style.top = (event.clientY + 12) + 'px';
    paintDropMark(hitTest(event.clientX, event.clientY));
  }

  function liftTab() {
    drag.moving = true;

    drag.ghost = document.createElement('div');
    drag.ghost.className = 'tab-ghost';
    drag.ghost.textContent = panes.get(drag.paneId).title;
    document.body.appendChild(drag.ghost);

    panes.forEach(function (pane) { pane.iframe.style.pointerEvents = 'none'; });
    columns.forEach(function (column) { column.tabbar.classList.add('dragging'); });
  }

  /*
   * Where the pointer is, phrased as what releasing there would do. Everything
   * is measured off the tab bars: a column's bar spans exactly the column's
   * track, so one rectangle gives both the column's width and where its
   * contents begin.
   */
  function hitTest(x, y) {
    if (!columns.length) return null;

    /*
     * Which column the pointer is over. A splitter counts as the column to its
     * right, and anything past either end as the column at that end -- test the
     * tracks literally and there is a four pixel strip, and two margins, where
     * the mark blinks out in the middle of a drag.
     */
    var index = columns.length - 1;
    for (var i = 0; i < columns.length; i += 1) {
      if (x < columns[i].tabbar.getBoundingClientRect().right) {
        index = i;
        break;
      }
    }

    var column = columns[index];
    var rect = column.tabbar.getBoundingClientRect();
    if (y < rect.top) return null;

    if (y < rect.bottom) {
      return { type: 'tabbar', column: column, index: insertionIndex(column, x) };
    }

    // Below the bar is the pane. The outer quarters offer a new column on that
    // side; the middle half joins this one.
    var fraction = (x - rect.left) / rect.width;
    if (fraction < 0.25) return { type: 'column', at: index };
    if (fraction > 0.75) return { type: 'column', at: index + 1 };
    return { type: 'merge', column: column };
  }

  /** The tabs a column shows while one of its own is in flight. */
  function settledTabs(column) {
    return column.order.filter(function (id) { return id !== drag.paneId; });
  }

  /** Which gap in `column`'s tab bar the pointer is nearest. */
  function insertionIndex(column, x) {
    var settled = settledTabs(column);
    for (var i = 0; i < settled.length; i += 1) {
      var rect = panes.get(settled[i]).tab.getBoundingClientRect();
      if (x < rect.left + rect.width / 2) return i;
    }
    return settled.length;
  }

  /** The x of the gap that `index` names, in window coordinates. */
  function gapX(column, index) {
    var settled = settledTabs(column);
    var bar = column.tabbar.getBoundingClientRect();
    if (!settled.length) return bar.left;
    if (index >= settled.length) {
      var last = panes.get(settled[settled.length - 1]).tab.getBoundingClientRect();
      return Math.min(last.right, bar.right);
    }
    return panes.get(settled[index]).tab.getBoundingClientRect().left;
  }

  function paintDropMark(target) {
    if (!target) {
      dropMark.hidden = true;
      return;
    }
    var box = panesEl.getBoundingClientRect();

    if (target.type === 'tabbar') {
      var bar = target.column.tabbar.getBoundingClientRect();
      dropMark.className = 'drop-mark line';
      dropMark.style.left = (gapX(target.column, target.index) - box.left - 1) + 'px';
      dropMark.style.top = (bar.top - box.top) + 'px';
      dropMark.style.width = '';
      dropMark.style.height = bar.height + 'px';
      dropMark.hidden = false;
      return;
    }

    var rect;
    var left;
    var width;
    if (target.type === 'merge') {
      rect = target.column.tabbar.getBoundingClientRect();
      left = rect.left;
      width = rect.width;
    } else {
      /*
       * A new column at `at` sits to the left of whatever is already there, so
       * it is that neighbour's left half being offered -- unless `at` is past
       * the end, in which case it is the right half of the last column.
       */
      var neighbour = columns[Math.min(target.at, columns.length - 1)];
      rect = neighbour.tabbar.getBoundingClientRect();
      width = rect.width / 2;
      left = target.at > columns.indexOf(neighbour) ? rect.left + width : rect.left;
    }

    dropMark.className = 'drop-mark area';
    dropMark.style.left = (left - box.left) + 'px';
    dropMark.style.top = (rect.bottom - box.top) + 'px';
    dropMark.style.width = width + 'px';
    dropMark.style.height = (box.bottom - rect.bottom) + 'px';
    dropMark.hidden = false;
  }

  function onDragEnd(event) {
    if (!drag) return;
    var target = drag.moving ? hitTest(event.clientX, event.clientY) : null;
    var paneId = drag.paneId;
    endDrag();
    if (!target) return;

    if (target.type === 'tabbar') {
      placePane(paneId, target.column, target.index);
      return;
    }
    if (target.type === 'merge') {
      placePane(paneId, target.column, target.column.order.length);
      return;
    }

    /*
     * A new column. When the tab is the only one where it stands its old column
     * dies as it leaves, so the count does not grow and the width test does not
     * apply -- the layout it lands in is one it already fits.
     */
    var from = columnOf(paneId);
    if (from.order.length > 1 && !roomForColumn()) return;
    placePane(paneId, makeColumn(target.at), 0);
  }

  function endDrag() {
    if (!drag) return;
    var tab = drag.tab;
    tab.removeEventListener('pointermove', onDragMove);
    tab.removeEventListener('pointerup', onDragEnd);
    tab.removeEventListener('pointercancel', endDrag);
    if (tab.hasPointerCapture(drag.pointerId)) tab.releasePointerCapture(drag.pointerId);
    if (drag.ghost) drag.ghost.remove();
    if (drag.moving) {
      panes.forEach(function (pane) { pane.iframe.style.pointerEvents = ''; });
      columns.forEach(function (column) { column.tabbar.classList.remove('dragging'); });
    }
    dropMark.hidden = true;
    drag = null;
  }

  // --------------------------------------------------------- column resize ---

  function beginColumnResize(event) {
    var bar = event.currentTarget;
    var index = splitters.indexOf(bar);
    if (index < 0 || index + 1 >= columns.length) return;
    event.preventDefault();

    var a = columns[index];
    var b = columns[index + 1];
    var startX = event.clientX;
    var startWidth = a.tabbar.getBoundingClientRect().width;
    var total = startWidth + b.tabbar.getBoundingClientRect().width;
    var share = a.weight + b.weight;
    // Too little room to honour the minimum on both sides: halve what there is
    // rather than refuse to move at all.
    var floor = Math.min(MIN_COLUMN, total / 2);

    bar.classList.add('dragging');
    bar.setPointerCapture(event.pointerId);
    panes.forEach(function (pane) { pane.iframe.style.pointerEvents = 'none'; });

    function move(moveEvent) {
      var want = startWidth + (moveEvent.clientX - startX);
      var width = Math.max(floor, Math.min(want, total - floor));
      a.weight = share * (width / total);
      b.weight = share - a.weight;
      paintTracks();
    }

    function done() {
      bar.removeEventListener('pointermove', move);
      bar.removeEventListener('pointerup', done);
      bar.removeEventListener('pointercancel', done);
      if (bar.hasPointerCapture(event.pointerId)) bar.releasePointerCapture(event.pointerId);
      bar.classList.remove('dragging');
      panes.forEach(function (pane) { pane.iframe.style.pointerEvents = ''; });
    }

    bar.addEventListener('pointermove', move);
    bar.addEventListener('pointerup', done);
    bar.addEventListener('pointercancel', done);
  }

  // -------------------------------------------------------------- tab menu ---

  var menu = null;
  var backdrop = null;

  function closeTabMenu() {
    if (menu) { menu.remove(); menu = null; }
    if (backdrop) { backdrop.remove(); backdrop = null; }
  }

  function separator() {
    var line = document.createElement('div');
    line.className = 'menu-separator';
    return line;
  }

  function menuItem(text, enabled, run) {
    var item = document.createElement('button');
    item.type = 'button';
    item.textContent = text;
    item.disabled = !enabled;
    item.onclick = function () {
      closeTabMenu();
      run();
    };
    return item;
  }

  /** A session beside the one it was copied from, on the same server. */
  function copySession(pane) {
    openTerminal(pane.server, pane.groupId, columnById(pane.columnId));
  }

  /** The tab's first child is its label; the second is the close button. */
  function retitle(pane, next) {
    if (!next) return;
    pane.title = next;
    pane.tab.querySelector('.tab-title').textContent = next;
  }

  function renameSession(pane) {
    ask(t('renameSession'), t('sessionNamePrompt'), pane.title).then(function (next) {
      if (next === null) return;
      retitle(pane, next);
    });
  }

  function openTabMenu(pane, x, y) {
    closeTabMenu();

    /*
     * The sheet under the menu is not decoration. The panes are frames, and a
     * click that lands inside one is a click this document never hears about --
     * without something covering them the menu would stay up while the user
     * typed into a terminal.
     */
    backdrop = document.createElement('div');
    backdrop.className = 'menu-backdrop';
    backdrop.addEventListener('pointerdown', closeTabMenu);
    backdrop.addEventListener('contextmenu', function (event) {
      event.preventDefault();
      closeTabMenu();
    });
    document.body.appendChild(backdrop);

    var allowed = canSplit(pane);
    menu = document.createElement('div');
    menu.className = 'context-menu';
    // The menu itself takes the focus, not its first item. Something in this
    // document has to have it or Escape never arrives -- the pane underneath is
    // a frame and keeps its own keystrokes -- but focusing a button draws a ring
    // around it, which reads as "this one is chosen" before anything is.
    menu.tabIndex = -1;
    menu.appendChild(menuItem(c('splitLeft'), allowed, function () {
      splitPane(pane.id, 'left');
    }));
    menu.appendChild(menuItem(c('splitRight'), allowed, function () {
      splitPane(pane.id, 'right');
    }));

    /*
     * Only a terminal has a session. A settings tab has nothing to copy and a
     * transfer tab is opened from the terminal it belongs to, so the two entries
     * are absent rather than greyed: a menu that is mostly disabled reads as
     * something being broken.
     */
    if (pane.kind === 'terminal' && pane.server) {
      menu.appendChild(separator());
      menu.appendChild(menuItem(t('copySession'), true, function () { copySession(pane); }));
      menu.appendChild(menuItem(t('renameSession'), true, function () { renameSession(pane); }));
    }

    /*
     * Closing, last, because it is the one entry here that throws work away.
     *
     * "Close others" is greyed rather than absent when this is the only tab:
     * absent would move "close this one" up under the pointer between one
     * right-click and the next, and a menu whose entries change position is a
     * menu you have to read every time.
     */
    menu.appendChild(separator());
    menu.appendChild(menuItem(c('closeTab'), true, function () { closePane(pane.id); }));
    menu.appendChild(menuItem(c('closeOthers'), panes.size > 1, function () {
      closeOthers(pane.id);
    }));

    document.body.appendChild(menu);

    // Measured after mounting, then nudged back inside the window: a menu
    // opened on the rightmost tab would otherwise hang off the edge.
    var rect = menu.getBoundingClientRect();
    menu.style.left = Math.max(0, Math.min(x, window.innerWidth - rect.width - 4)) + 'px';
    menu.style.top = Math.max(0, Math.min(y, window.innerHeight - rect.height - 4)) + 'px';

    menu.focus();
  }

  window.addEventListener('keydown', function (event) {
    if (event.key !== 'Escape') return;
    endDrag();
    closeTabMenu();
  });

  // --------------------------------------------------------------- dialogs ---

  /*
   * The two dialogs VS Code used to supply.
   *
   * Four of the server panel's nine messages were questions the extension passed
   * straight to `showInputBox` and `showWarningMessage`, and an undecorated
   * window has neither. The browser's own `prompt` and `confirm` are not a
   * substitute: they freeze the whole WebView while they are up, they cannot be
   * themed, and a Tauri window is not guaranteed to be allowed to show them at
   * all. So the names below shadow those globals on purpose -- reaching for the
   * blocking one by accident is then a mistake that cannot happen quietly.
   */
  function modal(options) {
    return new Promise(function (resolve) {
      var backdrop = document.createElement('div');
      backdrop.className = 'modal-backdrop';

      var box = document.createElement('div');
      box.className = 'modal';

      var heading = document.createElement('div');
      heading.className = 'modal-title';
      heading.textContent = options.title;

      var message = document.createElement('div');
      message.className = 'modal-message';
      message.textContent = options.message;
      box.append(heading, message);

      // Present only for a question that wants words back. Its absence is what
      // makes this a confirmation rather than a prompt.
      var input = null;
      if (options.value !== undefined) {
        input = document.createElement('input');
        input.className = 'modal-input';
        input.type = 'text';
        input.value = options.value;
        box.appendChild(input);
      }

      var cancel = document.createElement('button');
      cancel.type = 'button';
      cancel.className = 'modal-button';
      cancel.textContent = t('cancel');

      var accept = document.createElement('button');
      accept.type = 'button';
      accept.className = 'modal-button primary' + (options.danger ? ' danger' : '');
      accept.textContent = options.accept;

      var actions = document.createElement('div');
      actions.className = 'modal-actions';
      actions.append(cancel, accept);
      box.appendChild(actions);
      backdrop.appendChild(box);
      document.body.appendChild(backdrop);

      function close(result) {
        document.removeEventListener('keydown', onKey, true);
        backdrop.remove();
        resolve(result);
      }

      function done() {
        close(input ? input.value : true);
      }

      function onKey(event) {
        if (event.key === 'Escape') {
          event.preventDefault();
          close(null);
        } else if (event.key === 'Enter') {
          event.preventDefault();
          done();
        }
      }

      cancel.onclick = function () { close(null); };
      accept.onclick = done;
      // Only the sheet itself; a click that began inside the box and drifted out
      // over it -- selecting text, for instance -- is not a dismissal.
      backdrop.onpointerdown = function (event) {
        if (event.target === backdrop) close(null);
      };
      document.addEventListener('keydown', onKey, true);

      if (input) {
        input.focus();
        input.select();
      } else {
        accept.focus();
      }
    });
  }

  /** A line of text, or null if the user backed out. Blank counts as backing out. */
  function ask(title, message, value) {
    return modal({
      title: title,
      message: message,
      value: value,
      accept: c('ok')
    }).then(function (text) {
      return text !== null && text.trim() ? text.trim() : null;
    });
  }

  /** True only if the user pressed the button the question named. */
  function confirm(title, message, accept) {
    return modal({
      title: title,
      message: message,
      accept: accept,
      danger: true
    }).then(function (result) {
      return result === true;
    });
  }

  function send(paneId, message) {
    var pane = panes.get(paneId);
    if (!pane || !pane.iframe.contentWindow) return;
    pane.iframe.contentWindow.postMessage({ __tshell: 'host', payload: message }, '*');
  }

  /** Every frame in the window, the sidebar included. */
  function broadcast(message) {
    var aside = sidebar.querySelector('iframe');
    if (aside && aside.contentWindow) {
      aside.contentWindow.postMessage({ __tshell: 'host', payload: message }, '*');
    }
    panes.forEach(function (pane) { send(pane.id, message); });
  }

  /*
   * Switching the theme used to reload every frame, which was both slow and a
   * lie: an iframe whose URL differs only after the '#' is not reloaded at all,
   * so open panes kept the old colours. Now the change is announced and each
   * frame's host.js applies it in place -- which is also the only way xterm's
   * canvas can follow along without losing the scrollback.
   */
  function applyTheme(next) {
    theme = next;
    if (next === 'light') document.documentElement.setAttribute('data-theme', 'light');
    else document.documentElement.removeAttribute('data-theme');
    try {
      localStorage.setItem('tshell:theme', next);
    } catch (error) {
      // Remembering the choice is a convenience, not a requirement.
    }
    broadcast({ type: 'theme', theme: next });
    // A built-in scheme has a dark half and a light half, so the theme just
    // changed which sixteen colours the terminals should be painting with.
    broadcastAppearance();
  }

  /*
   * The terminals' appearance, pushed down as one object.
   *
   * Broadcast rather than reloaded: every open terminal takes the new colours
   * on the next frame, scrollback included, because assigning xterm's
   * `options.theme` repaints the whole buffer. Nothing reconnects and nothing
   * is lost, which is what makes this worth having as a live setting rather
   * than one that asks you to reopen your tabs.
   */
  function appearance() {
    return window.tshellSchemes.appearance(window.tshellSchemes.selected(), theme);
  }

  function broadcastAppearance() {
    broadcast({ type: 'appearance', appearance: appearance() });
  }

  /**
   * Take an edited schemes file and show it, without writing it.
   *
   * This is what a colour picker being dragged sends: the terminals repaint on
   * every frame of the drag, and the disk is not touched until the drag ends.
   */
  function applySchemes(file) {
    window.tshellSchemes.load(file);
    broadcastAppearance();
  }

  /**
   * Take an edited schemes file, write it, and hand back what was written.
   *
   * The reply carries the *normalized* file rather than the one that was sent:
   * Rust is what decides that `0e8563` is `#0E8563`, and the settings page
   * redrawing from anything else would be showing a value the file does not
   * hold. A refusal comes back as an error and leaves the file alone.
   */
  function saveSchemes(paneId, file) {
    applySchemes(file);
    if (schemesError) {
      // Refusing to read it and then writing over it anyway is how a stray
      // comma costs somebody every scheme they made.
      send(paneId, { type: 'schemes', file: window.tshellSchemes.file(), error: schemesError });
      return;
    }
    invoke('schemes_save', { file: file })
      .then(function (written) {
        applySchemes(written);
        send(paneId, { type: 'schemes', file: written, error: '' });
      })
      .catch(function (error) {
        send(paneId, {
          type: 'schemes',
          file: window.tshellSchemes.file(),
          error: String(error)
        });
      });
  }

  /*
   * The monospaced fonts this machine has, which only Rust can enumerate -- the
   * web platform has no way to ask. Fetched once, when a settings page first
   * wants them, because the first call reads a table out of every font file on
   * the machine and most sessions never open settings at all.
   */
  function sendFonts(paneId) {
    if (fontList) {
      send(paneId, { type: 'fonts', fonts: fontList });
      return;
    }
    invoke('fonts_monospace')
      .then(function (fonts) {
        fontList = fonts || [];
        send(paneId, { type: 'fonts', fonts: fontList });
      })
      .catch(function (error) {
        // A picker without suggestions is still a picker: the field takes
        // anything typed into it either way.
        console.warn('[shell] the installed fonts could not be listed:', error);
        fontList = [];
        send(paneId, { type: 'fonts', fonts: fontList });
      });
  }

  /** One settings tab at a time; asking again brings the open one forward. */
  function openSettings() {
    var open = null;
    panes.forEach(function (pane) { if (pane.kind === 'settings') open = pane; });
    if (open) { activate(open.id); return; }
    openPane('settings', {
      page: 'settings/settings.html',
      title: language === 'en-US' ? 'Settings' : '设置',
      bootstrap: {
        strings: pageStrings(),
        language: language,
        theme: theme,
        // The whole schemes file, not a resolved appearance: this page is the
        // one that edits it, and it needs the parts rather than the result.
        schemes: window.tshellSchemes.file(),
        schemesError: schemesError,
        // Where the config and the secrets ended up. Read once at boot and not
        // changeable from anywhere, so the bootstrap is the whole of it.
        storage: panel.storage || {}
      }
    });
  }

  // ------------------------------------------------------------ bootstraps ---

  function pageStrings() {
    return strings[language] || {};
  }

  /*
   * How many copies of each base name have been handed out. Never decremented,
   * as it was not in the extension: reusing the number of a tab that has been
   * closed makes two different sessions share a name in the same afternoon,
   * which is worse than counting past the tabs that are actually open.
   */
  var titleCounts = new Map();

  function titleTaken(title) {
    var taken = false;
    panes.forEach(function (pane) {
      if (pane.kind === 'terminal' && pane.title === title) taken = true;
    });
    return taken;
  }

  /*
   * The first session on a server is just its name; every one after that gets
   * `name(n)`, which is what the extension did for a copied session. It did not
   * do it for a second double-click, so opening the same server nine times gave
   * nine tabs with the same label and no way to tell them apart. Everything that
   * opens a terminal comes through here now.
   */
  function terminalTitle(base) {
    if (!titleTaken(base)) return base;
    var n = titleCounts.get(base) || 0;
    do { n += 1; } while (titleTaken(base + '(' + n + ')'));
    titleCounts.set(base, n);
    return base + '(' + n + ')';
  }

  function openTerminal(server, groupId, column) {
    var base = server.name || server.host;
    var pane = openPane('terminal', {
      page: 'terminal/terminal.html',
      title: terminalTitle(base),
      server: server,
      column: column,
      bootstrap: {
        strings: pageStrings(),
        language: language,
        aiEnabled: aiEnabled,
        appearance: appearance()
      }
    });
    pane.groupId = groupId;
    /*
     * What the pty is opened at, until the page has laid itself out and sent
     * its real size back. The same pair the extension started from.
     */
    pane.cols = 120;
    pane.rows = 36;
    // What the pty was last told, which starts as nothing: the size the pty is
    // opened at is recorded by `connect`, not assumed here.
    pane.ptyCols = 0;
    pane.ptyRows = 0;
    pane.connected = false;
    pane.connecting = false;
    return pane;
  }

  function openTransfer(server, groupId, beside) {
    var pane = openPane('transfer', {
      page: 'transfer/transfer.html',
      title: t('fileTransfer') + ' — ' + (server.name || server.host),
      server: server,
      column: columnRightOf(beside),
      bootstrap: {
        strings: pageStrings(),
        language: language,
        defaultEncoding: server.encoding === 'gb18030' ? 'gb2312' : 'utf8',
        showHiddenFiles: false
      }
    });
    pane.groupId = groupId;
    // Where each pane is looking. The page draws the path but does not own it:
    // refresh and "up" are answered from here, so this is the copy that counts.
    pane.paths = { local: '', remote: '' };

    /*
     * Watch for the session dying.
     *
     * The terminal finds out on its own -- it has a channel with something
     * coming down it -- but a transfer page is silent between clicks, and
     * checking only when the user next asks for a listing means a connection
     * can be dead for ten minutes with the page looking perfectly healthy.
     * The extension had an event for this; russh has a flag, so it is polled.
     * The call is a boolean read behind a mutex, four times a minute.
     */
    pane.watch = setInterval(function () {
      invoke('transfer_died', { pane: pane.id })
        .then(function (died) {
          if (died) transferLog(pane, t('transferClosedRetry'));
        })
        .catch(function () {});
    }, 4000);

    return pane;
  }

  // ------------------------------------------------------------- transfers ---

  /*
   * Every transfer command names the server it is about. A tab could have kept a
   * copy of one, but a tab outlives an edit to the config, and the copy would go
   * on pointing at the old host long after the user had corrected it.
   */
  function transferArgs(pane, side, extra) {
    var args = {
      pane: pane.id,
      side: side,
      groupId: pane.groupId,
      serverId: pane.server.id
    };
    Object.keys(extra || {}).forEach(function (key) { args[key] = extra[key]; });
    return args;
  }

  /** The page keeps a log pane; anything that goes wrong is said there. */
  function transferLog(pane, words) {
    send(pane.id, { type: 'log', text: words });
  }

  function transferFailed(pane, error) {
    transferLog(pane, t(String((error && error.message) || error)));
  }

  function browse(pane, side, path) {
    /*
     * The remote pane connects on its first request, and the log is where the
     * user finds out. The extension said the same three things about a terminal;
     * an SFTP session that is quietly taking twenty seconds to open, with a pane
     * that just sits empty, is the case this exists for.
     */
    var opening = side === 'remote' && !pane.remoteReady;
    if (opening) transferLog(pane, t('connecting') + ' ' + who(pane) + ' ...');

    return invoke('transfer_list', transferArgs(pane, side, { path: path || '' }))
      .then(function (listing) {
        // The watcher has usually said the connection went already, so this
        // only reports the recovery. When the drop and the click land in the
        // same few seconds it is the one that says anything at all.
        if (listing.reconnected) {
          transferLog(pane, t('connected') + ': ' + who(pane));
        }
        if (opening) {
          pane.remoteReady = true;
          transferLog(pane, t('connected') + ': ' + who(pane));
          paintStatus();
        }
        pane.paths[side] = listing.path;
        if (side === 'local') {
          try {
            localStorage.setItem('tshell:localDir', listing.path);
          } catch (error) {
            // Remembering the place is a convenience, not a requirement.
          }
        }
        send(pane.id, {
          type: 'list',
          side: side,
          path: listing.path,
          entries: listing.entries,
          canGoUp: listing.canGoUp
        });
      })
      .catch(function (error) {
        // The pane keeps its last good path rather than going blank.
        send(pane.id, { type: 'listFailed', side: side });
        if (opening) transferLog(pane, t('connectFailed') + ': ' + reason(error));
        else transferFailed(pane, error);
      });
  }

  /** The last segment of a path, under either side's separators. */
  function baseName(path) {
    var parts = String(path).split(/[\\/]/).filter(Boolean);
    return parts.length ? parts[parts.length - 1] : String(path);
  }

  /*
   * Which reader answers is decided by Rust, from the extension, so that the
   * highlighter the panel picks and the parser that produced the data are never
   * working from different ideas about what the file is.
   */
  function openPreview(pane, side, path, encoding) {
    if (!path) return;

    invoke('preview_language', { path: path }).then(function (language) {
      if (language === 'dbf') {
        return invoke('preview_dbf', transferArgs(pane, side, {
          path: path,
          encoding: encoding,
          recordOffset: 0
        })).then(function (chunk) {
          send(pane.id, {
            type: 'dbfPreview',
            side: side,
            path: path,
            name: baseName(path),
            language: 'dbf',
            encoding: encoding,
            fields: chunk.fields,
            rows: chunk.rows,
            recordCount: chunk.recordCount,
            nextRecord: chunk.nextRecord,
            done: chunk.done
          });
          transferLog(pane, t('previewing') + ': ' + path);
        });
      }

      return invoke('preview_text', transferArgs(pane, side, {
        path: path,
        encoding: encoding,
        offset: 0
      })).then(function (chunk) {
        send(pane.id, {
          type: 'textPreview',
          side: side,
          path: path,
          name: baseName(path),
          language: language,
          encoding: encoding,
          content: chunk.content,
          unsupported: chunk.binary,
          message: chunk.binary ? t('unsupportedBinaryPreview') : '',
          done: chunk.done,
          nextOffset: chunk.nextOffset,
          totalSize: chunk.size
        });
        transferLog(pane, t(chunk.binary ? 'unsupportedBinaryPreview' : 'previewing') + ': ' + path);
      });
    }).catch(function (error) { transferFailed(pane, error); });
  }

  /*
   * A running transfer, from the drop to the summary.
   *
   * Everything in between arrives on a channel: the scan totals, the progress
   * ticks, and the questions about files that are already there. The command's
   * own promise resolves once, at the end, with what happened.
   */
  function startTransfer(pane, side, items, targetDir) {
    var channel = new tauri.Channel();
    channel.onmessage = function (event) { onTransferEvent(pane, event); };

    send(pane.id, { type: 'transferState', active: true });

    invoke('transfer_start', transferArgs(pane, side, {
      items: items,
      targetDir: targetDir,
      onEvent: channel
    })).then(function (summary) {
      var parts = [
        summary.completed + ' ' + t('summaryCompleted'),
        summary.skipped + ' ' + t('summarySkipped'),
        summary.failed + ' ' + t('summaryFailed')
      ];
      transferLog(pane, t('transferSummary', parts.join(' · ')));
      if (summary.cancelled) transferLog(pane, t('transferCancelled'));
      // Both panes: the source may have been the one that changed, and after a
      // download the local side is the one with something new in it.
      browse(pane, 'local', pane.paths.local);
      browse(pane, 'remote', pane.paths.remote);
    }).catch(function (error) {
      transferFailed(pane, error);
    }).then(function () {
      send(pane.id, { type: 'transferState', active: false });
      send(pane.id, { type: 'progressEnd' });
    });
  }

  function onTransferEvent(pane, event) {
    if (!event || typeof event !== 'object') return;

    if (event.kind === 'scanning') {
      send(pane.id, {
        type: 'progress',
        phase: 'scanning',
        overall: { totalFiles: event.totalFiles, totalBytes: event.totalBytes },
        current: {}
      });
      return;
    }

    if (event.kind === 'progress') {
      send(pane.id, {
        type: 'progress',
        phase: 'transferring',
        overall: {
          doneFiles: event.doneFiles,
          totalFiles: event.totalFiles,
          doneBytes: event.doneBytes,
          totalBytes: event.totalBytes
        },
        current: { name: event.name, transferred: event.transferred, total: event.total }
      });
      return;
    }

    if (event.kind === 'conflict') {
      send(pane.id, {
        type: 'conflict',
        id: event.id,
        name: event.name,
        sourcePath: event.sourcePath,
        targetPath: event.targetPath,
        sourceKind: event.sourceKind,
        targetKind: event.targetKind,
        sourceSize: event.sourceSize,
        targetSize: event.targetSize,
        sourceModifiedAt: event.sourceModifiedAt,
        targetModifiedAt: event.targetModifiedAt,
        remaining: event.remaining
      });
      return;
    }

    if (event.kind === 'log') {
      // The key is translated here; the detail is paths, which no table holds.
      transferLog(pane, event.detail ? t(event.key) + ': ' + event.detail : t(event.key));
      return;
    }
    // `end` needs nothing done here: the command's promise is about to settle,
    // and that is where the panel is closed.
  }

  function fromTransfer(pane, message) {
    var side = message.side === 'remote' ? 'remote' : 'local';

    switch (message.type) {
      case 'ready':
        /*
         * The local pane reopens where it was last left, and falls back to the
         * home directory. Kept in localStorage beside the theme rather than in
         * the config file: it is where a window happened to be looking, not
         * something the user configured, and writing tshell.config.json on every
         * double-click would churn a file meant to be read by hand.
         *
         * The remote default is the login directory, which only the far side
         * knows. Both fallbacks resolve in Rust, where the rules about roots and
         * separators already live.
         */
        var remembered = '';
        try {
          remembered = localStorage.getItem('tshell:localDir') || '';
        } catch (error) {
          // No store, no memory. The home directory stands.
        }
        if (remembered) browse(pane, 'local', remembered);
        else invoke('transfer_default_local').then(function (home) {
          browse(pane, 'local', home);
        });
        browse(pane, 'remote', '');
        return;

      case 'list':
        browse(pane, side, message.path || '');
        return;

      case 'refresh':
        browse(pane, side, pane.paths[side]);
        return;

      case 'up':
        invoke('transfer_parent', { side: side, path: pane.paths[side] })
          .then(function (parent) { browse(pane, side, parent); });
        return;

      case 'setHidden':
        invoke('transfer_set_hidden', transferArgs(pane, side, { value: !!message.value }))
          .then(function () { browse(pane, side, pane.paths[side]); });
        return;

      case 'newFolder':
        ask(t('newFolder'), t('newFolderPrompt'), '').then(function (name) {
          if (name === null) return;
          invoke('transfer_join', { side: side, base: pane.paths[side], name: name })
            .then(function (path) {
              return invoke('transfer_make_dir', transferArgs(pane, side, { path: path }));
            })
            .then(function () { browse(pane, side, pane.paths[side]); })
            .catch(function (error) { transferFailed(pane, error); });
        });
        return;

      case 'renameEntry':
        ask(t('rename'), t('renamePrompt'), baseName(message.path)).then(function (name) {
          if (name === null) return;
          invoke('transfer_join', { side: side, base: pane.paths[side], name: name })
            .then(function (to) {
              return invoke('transfer_rename', transferArgs(pane, side, {
                from: message.path,
                to: to
              }));
            })
            .then(function () { browse(pane, side, pane.paths[side]); })
            .catch(function (error) { transferFailed(pane, error); });
        });
        return;

      case 'deleteEntries':
        var items = message.items || [];
        if (!items.length) return;
        var paths = items.map(function (item) { return item.path; });
        var what = items.length === 1 ? baseName(paths[0]) : String(items.length);
        var question = items.length === 1
          ? t('deleteEntryConfirm', what)
          : t('deleteEntriesConfirm', what);
        confirm(t('delete'), question, t('delete')).then(function (yes) {
          if (!yes) return;
          invoke('transfer_remove', transferArgs(pane, side, { paths: paths }))
            .then(function () { browse(pane, side, pane.paths[side]); })
            .catch(function (error) { transferFailed(pane, error); });
        });
        return;

      case 'copyText':
        navigator.clipboard.writeText(message.text || '').catch(function () {});
        return;

      case 'transfer':
        /*
         * Only a drag onto a directory row names its own destination. The menu
         * entry does not, and the answer then is the other pane's current
         * directory -- which is the whole point of a two-pane transfer.
         *
         * Defaulting to '' instead, as this did, sent downloads to a bare
         * filename in whatever the process's working directory happened to be,
         * and uploads to `/name` on the server, which is why they came back as
         * permission denied.
         */
        var into = message.targetDir || pane.paths[side === 'local' ? 'remote' : 'local'];
        startTransfer(pane, side, message.items || [], into);
        return;

      case 'cancelTransfer':
        invoke('transfer_cancel', { pane: pane.id });
        return;

      case 'conflictResult':
        invoke('transfer_answer', {
          pane: pane.id,
          id: message.id,
          choice: message.choice || 'skip'
        });
        return;

      case 'openPreview':
        openPreview(pane, side, message.path || '', message.encoding || '');
        return;

      case 'loadTextChunk':
        invoke('preview_text', transferArgs(pane, side, {
          path: message.path,
          encoding: message.encoding || '',
          offset: message.offset || 0
        })).then(function (chunk) {
          send(pane.id, {
            type: 'textChunk',
            side: side,
            path: message.path,
            encoding: message.encoding,
            content: chunk.content,
            done: chunk.done,
            nextOffset: chunk.nextOffset,
            totalSize: chunk.size
          });
        }).catch(function (error) { transferFailed(pane, error); });
        return;

      case 'loadDbfChunk':
        invoke('preview_dbf', transferArgs(pane, side, {
          path: message.path,
          encoding: message.encoding || '',
          recordOffset: message.recordOffset || 0
        })).then(function (chunk) {
          send(pane.id, {
            type: 'dbfChunk',
            side: side,
            path: message.path,
            encoding: message.encoding,
            rows: chunk.rows,
            nextRecord: chunk.nextRecord,
            done: chunk.done
          });
        }).catch(function (error) { transferFailed(pane, error); });
        return;

      case 'clearLog':
        return;
    }

    console.info('[shell] transfer pane, arrives with a later step:', message.type, message);
  }

  /*
   * The assistant, over one terminal.
   *
   * It borrows that terminal rather than opening its own connection: the
   * commands are typed into the shell the user is watching, so `cd`, `export`
   * and `sudo` behave exactly as they do when typed by hand. That is why this
   * takes the terminal's pane id and not just its server -- see `ai/shell.rs`.
   *
   * The id is minted before the tab, because the bootstrap the page loads with
   * is what `ai_open` answers, and `ai_open` has to be told which pane it is
   * answering for.
   */
  function openChat(terminalPane) {
    var server = terminalPane.server;
    if (!server) return;
    var paneId = 'chat-' + paneEpoch + '-' + (paneSeq += 1);
    var channel = new tauri.Channel();

    return invoke('ai_open', {
      pane: paneId,
      terminal: terminalPane.id,
      groupId: terminalPane.groupId,
      serverId: server.id,
      out: channel
    }).then(function (boot) {
      var pane = openPane('chat', {
        paneId: paneId,
        page: 'chat/chat.html',
        title: t('agentTitle') + ' — ' + (server.name || server.host),
        server: server,
        /*
         * Worked out here rather than before the request, because it can create
         * a column: doing that first would leave an empty one behind whenever
         * `ai_open` failed, and an empty column is a stripe of tab bar the user
         * has to close by hand.
         */
        column: columnRightOf(terminalPane),
        groupId: terminalPane.groupId,
        terminal: terminalPane.id,
        bootstrap: {
          strings: pageStrings(),
          language: language,
          host: boot.host,
          thinking: boot.thinking,
          mode: boot.mode,
          model: boot.model
        }
      });
      // Attached only once the pane exists, so nothing Rust pushes arrives
      // before there is a frame to put it in.
      channel.onmessage = function (message) { toChat(pane, message); };
      return pane;
    }).catch(function (error) {
      console.error('[shell] the assistant would not open:', error);
    });
  }

  /*
   * Rust -> the chat page.
   *
   * Almost everything is forwarded unchanged: `chat.js` is the extension's, and
   * the messages were shaped in `ai/types.rs` to be exactly what it already
   * reads. The two that are not are the ones where only the shell knows the
   * answer -- the string table for a memory window's headings, and whether the
   * feature is switched on at all.
   */
  function toChat(pane, message) {
    if (!message || !message.type) return;
    /*
     * The directory the file dialog is offering to trust, kept here rather than
     * sent back with the answer.
     *
     * `chat.js` replies with the answer alone -- it is the extension's page and
     * knows nothing about a trust store -- so the one side that saw both the
     * question and the answer has to hold the connection between them.
     */
    if (message.type === 'confirmFile') pane.trustDir = message.trustDir || '';
    if (message.type === 'confirm') pane.trustDir = '';
    // The tab is the shell's, so the one thing Rust cannot do for itself is
    // rename it. Sent once, when a conversation first gets a title.
    if (message.type === 'title') { retitle(pane, message.text); return; }
    if (message.type === 'restore') pane.chatId = message.id || '';
    if (message.type === 'cleared') pane.chatId = '';
    send(pane.id, message);
  }

  /*
   * The chat page -> Rust.
   *
   * Every one of these is a command and nothing else: the page owns the words,
   * the loop owns the work, and this is only the wire between them. Anything
   * that answers with a list posts that list straight back, because the window
   * that asked for it is already open and waiting.
   */
  function fromChat(pane, message) {
    var id = pane.id;
    switch (message.type) {
      case 'ready':
        // The page has drawn itself and is asking what it is looking at.
        return;

      case 'send':
        invoke('ai_send', { pane: id, text: message.text || '' });
        return;
      case 'stop':
        invoke('ai_stop', { pane: id });
        return;

      /*
       * `trust` is a third answer on the file dialog, not a second yes: it says
       * yes AND stops the question being asked again for that directory. So it
       * is two calls, in that order -- the directory is trusted first, so the
       * row saying so is drawn above the change it let through.
       */
      case 'confirm':
        if (message.answer === 'trust' && pane.trustDir) {
          invoke('ai_trust_add', { pane: id, dir: pane.trustDir })
            .then(function () {
              invoke('ai_answer', { pane: id, id: message.id, ok: true });
            });
          return;
        }
        invoke('ai_answer', { pane: id, id: message.id, ok: message.answer === 'yes' });
        return;

      case 'history':
        invoke('ai_history').then(function (data) {
          send(id, { type: 'history', items: data.items, current: pane.chatId || '' });
        });
        return;
      case 'newChat':
        invoke('ai_new_chat', { pane: id });
        return;
      case 'loadChat':
        invoke('ai_load_chat', { pane: id, id: message.id }).then(function (data) {
          pane.chatId = data.id;
          send(id, data);
        });
        return;
      case 'deleteChat':
        invoke('ai_delete_chat', { pane: id, id: message.id }).then(function () {
          return invoke('ai_history');
        }).then(function (data) {
          send(id, { type: 'history', items: data.items, current: pane.chatId || '' });
        });
        return;

      case 'memoryList':
        paintMemory(pane);
        return;
      case 'undoMemory':
        invoke('ai_memory_undo', { token: message.token }).then(function (ok) {
          send(id, { type: 'memoryUndone', token: message.token, ok: ok });
        });
        return;
      case 'openMemory':
        invoke('ai_reveal', {
          what: message.scope === 'global' ? 'memoryGlobal' : 'memoryServer',
          pane: id
        });
        return;

      case 'trustList':
        invoke('ai_trust_list', { pane: id }).then(function (data) {
          send(id, { type: 'trustList', dirs: data.dirs, mode: data.mode });
        });
        return;
      case 'removeTrust':
        invoke('ai_trust_remove', { pane: id, dir: message.dir }).then(function () {
          return invoke('ai_trust_list', { pane: id });
        }).then(function (data) {
          send(id, { type: 'trustList', dirs: data.dirs, mode: data.mode });
        });
        return;
      case 'openTrust':
        invoke('ai_reveal', { what: 'trust', pane: id });
        return;

      case 'skillList':
        paintSkills(pane);
        return;
      case 'toggleSkill':
        // The row says which state it puts you in, so the toggle is worked out
        // from what is on screen rather than sent as a state the page guessed.
        invoke('ai_skill_list').then(function (data) {
          var found = (data.items || []).find(function (item) { return item.id === message.id; });
          return invoke('ai_skill_toggle', { id: message.id, on: !!(found && found.disabled) });
        }).then(function () { paintSkills(pane); });
        return;
      case 'openSkill':
      case 'openSkillFolder':
        invoke('ai_reveal', { what: 'skills' });
        return;

      case 'modelList':
        paintModels(pane);
        return;
      case 'addModel':
        invoke('ai_model_add', {
          baseUrl: message.baseUrl,
          model: message.model,
          apiKey: message.apiKey || ''
        }).then(function () { paintModels(pane); broadcastAiState(); });
        return;
      case 'deleteModel':
        invoke('ai_model_delete', { id: message.id })
          .then(function () { paintModels(pane); broadcastAiState(); });
        return;
      case 'selectModel':
        invoke('ai_model_select', { id: message.id })
          .then(function () { paintModels(pane); broadcastAiState(); });
        return;

      case 'setMode':
        invoke('ai_set_mode', { mode: message.mode }).then(broadcastAiState);
        return;
      case 'setThinking':
        invoke('ai_set_thinking', {
          enabled: 'enabled' in message ? message.enabled : null,
          effort: 'effort' in message ? message.effort : null,
          show: 'show' in message ? message.show : null
        }).then(broadcastAiState);
        return;
    }

    console.info('[shell] chat pane, not wired:', message.type, message);
  }

  function paintMemory(pane) {
    Promise.all([
      invoke('ai_memory_list', { pane: pane.id }),
      invoke('ai_settings_read')
    ]).then(function (both) {
      var lists = both[0];
      var settings = both[1];
      send(pane.id, {
        type: 'memoryList',
        enabled: settings.memory.enabled,
        serverName: pane.server ? (pane.server.name || pane.server.host) : '',
        global: lists.global,
        server: lists.server
      });
    });
  }

  function paintSkills(pane) {
    Promise.all([invoke('ai_skill_list'), invoke('ai_settings_read')]).then(function (both) {
      send(pane.id, {
        type: 'skillList',
        enabled: both[1].skills.enabled,
        items: both[0].items
      });
    });
  }

  function paintModels(pane) {
    invoke('ai_model_list').then(function (data) {
      send(pane.id, { type: 'modelList', active: data.active, items: data.items });
    });
  }

  /*
   * The toolbar of every open panel, after a setting that belongs to all of them.
   *
   * Mode, thinking and the model are global -- they are a posture, not a
   * property of one conversation -- so a switch made in one panel is the truth
   * in the others a moment later. Without this the panel that made the change
   * would be the only one showing it.
   */
  function broadcastAiState() {
    return invoke('ai_settings_read').then(function (settings) {
      var active = (settings.models || []).find(function (model) {
        return model.id === settings.activeModelId;
      });
      var state = {
        type: 'aiState',
        thinking: settings.thinking,
        mode: settings.agent.mode,
        model: active ? active.model : ''
      };
      panes.forEach(function (pane) {
        if (pane.kind === 'chat') send(pane.id, state);
      });
    });
  }

  // -------------------------------------------------------------- sessions ---

  /*
   * Rust pushes terminal output as raw UTF-8 down a per-session channel, never as
   * JSON -- see src-tauri/src/ssh.rs for why that is a requirement rather than a
   * refinement. Anything that arrives on the channel that is *not* bytes is a
   * status object, which is what tells the two apart without a second channel.
   */
  var utf8 = new TextDecoder();

  /** A line from tshell rather than from the remote shell, dimmed to say so. */
  function note(pane, words) {
    send(pane.id, { type: 'output', data: '\x1b[2m' + words + '\x1b[0m\r\n' });
  }

  function who(pane) {
    return pane.server.username + '@' + pane.server.host + ':' + pane.server.port;
  }

  function onSessionEvent(pane, payload) {
    if (payload instanceof ArrayBuffer) {
      send(pane.id, { type: 'output', data: utf8.decode(payload) });
      return;
    }
    if (!payload || typeof payload !== 'object') return;

    if (payload.kind === 'opened') {
      pane.connected = true;
      /*
       * Clear here and nowhere else. The remote's own greeting -- `Last login:`
       * and the prompt -- is the first thing that should be on the screen, and
       * this frame is guaranteed to be ahead of it: Rust pushes it down the same
       * ordered channel before the output pump exists. Clearing when the command
       * resolves instead would be a race against that first flush.
       */
      send(pane.id, { type: 'connected', clear: true });
      paintStatus();
      return;
    }
    if (payload.kind === 'closed') {
      pane.connected = false;
      note(pane, t('connectionClosedRetryEnter'));
      paintStatus();
    }
  }

  function connect(pane) {
    if (pane.connecting || pane.connected) return;
    pane.connecting = true;
    paintStatus();

    var channel = new tauri.Channel();
    channel.onmessage = function (payload) { onSessionEvent(pane, payload); };

    note(pane, t('connecting') + ' ' + who(pane) + ' ...');

    // The pty is opened at whatever the page has most recently reported, so that
    // is what it is on record as having been told.
    pane.ptyCols = pane.cols;
    pane.ptyRows = pane.rows;

    invoke('terminal_open', {
      pane: pane.id,
      groupId: pane.groupId,
      serverId: pane.server.id,
      cols: pane.cols,
      rows: pane.rows,
      onEvent: channel
    }).then(function () {
      // Success is announced by the `opened` frame above, which has already
      // cleared the screen. Saying so again here would only put a line back on
      // top of the greeting that was just made room for.
      pane.connecting = false;
      pane.connected = true;
      paintStatus();
      // Asked for from the server panel, where there was no terminal to hang
      // it on. Now there is one and it is up.
      if (pane.assistantWhenReady) {
        pane.assistantWhenReady = false;
        openChat(pane);
      }
    }).catch(function (error) {
      pane.ptyCols = 0;
      pane.ptyRows = 0;
      pane.connecting = false;
      pane.connected = false;
      note(pane, t('connectFailed') + ': ' + reason(error));
      paintStatus();
    });
  }

  /*
   * Rust rejects with a string-table key where it has something specific to say
   * -- `authFailed` -- and with the underlying error where it
   * does not. Both tables return what they do not recognise unchanged, so a real
   * message from the network stack arrives intact rather than as a key name.
   */
  function reason(error) {
    var text = String((error && error.message) || error);
    var local = (chromeText[language] || {})[text];
    return local !== undefined ? local : t(text);
  }

  // ----------------------------------------------------------- page router ---

  /*
   * The server panel, wired to the config.
   *
   * Four of these used to be answered by VS Code itself: `showInputBox` for the
   * two names, `showWarningMessage` for the two deletions. An undecorated window
   * has neither, so the shell draws its own -- see `ask` and `confirm` below.
   * That is the whole of what changed on this side; the page still sends exactly
   * the nine messages it sent to the extension.
   */
  function fromServers(message) {
    var group;

    switch (message.type) {
      case 'openTerminal':
        var server = findServer(message.groupId, message.serverId);
        if (server) openTerminal(server, message.groupId);
        return;

      case 'openTransfer':
        // The file panel keeps a connection of its own, so it needs a server
        // and nothing else -- no terminal has to exist first.
        var forTransfer = findServer(message.groupId, message.serverId);
        if (forTransfer) openTransfer(forTransfer, message.groupId);
        return;

      /*
       * The assistant is different: it does its work *through* a terminal's
       * shell, so one has to exist and be up before there is anything to
       * attach to. Asked for from the server panel there is no terminal yet,
       * so open one and leave a note on it; `connect` reads the note once Rust
       * has the session registered.
       *
       * Opening the assistant against a pane that is still dialling would have
       * `ai_open` look up a session that is not there yet.
       */
      case 'openAssistant':
        var forAssistant = findServer(message.groupId, message.serverId);
        if (!forAssistant) return;
        openTerminal(forAssistant, message.groupId).assistantWhenReady = true;
        return;

      case 'addServer':
      case 'updateServer':
        panelCommand(message.type === 'addServer' ? 'add_server' : 'update_server', {
          groupId: message.groupId,
          server: message.server,
          password: message.password || '',
          privateKeyPassphrase: message.privateKeyPassphrase || ''
        }).then(adoptEdits);
        return;

      /*
       * A drop in the tree. Nothing here asks a question first -- a drag is a
       * gesture the user can abandon by not letting go, so a dialog on top of
       * it would be asking them to confirm something they already confirmed --
       * and both are undone by dragging the row back.
       */
      case 'moveGroup':
        panelCommand('move_group', {
          groupId: message.groupId,
          toIndex: message.toIndex
        });
        return;

      case 'moveServer':
        panelCommand('move_server', {
          fromGroupId: message.fromGroupId,
          serverId: message.serverId,
          toGroupId: message.toGroupId,
          toIndex: message.toIndex
        }).then(function () {
          /*
           * A tab remembers the group it was opened from, and a cross-group
           * drop has just made that memory wrong. It has to be corrected before
           * adoptEdits, which looks the server up by (groupId, serverId) and
           * would find nothing -- and it has to be corrected at all, because
           * the same stale pair is what a reconnect and a reload of the
           * encoding are dialled with.
           *
           * Asked of the state that came back rather than of the message that
           * asked for the move: panelCommand swallows its own errors, so this
           * runs whether or not the move happened, and only the new state knows
           * which of those it was.
           */
          if (findServer(message.toGroupId, message.serverId)) {
            panes.forEach(function (pane) {
              if (pane.server && pane.server.id === message.serverId) {
                pane.groupId = message.toGroupId;
              }
            });
          }
          adoptEdits();
        });
        return;

      case 'requestAddGroup':
        ask(t('addGroup'), t('groupNamePrompt'), t('newGroup')).then(function (name) {
          if (name !== null) panelCommand('add_group', { name: name });
        });
        return;

      case 'requestRenameGroup':
        group = findGroup(message.groupId);
        if (!group) return;
        ask(t('renameGroup'), t('newGroupNamePrompt'), group.name).then(function (name) {
          if (name !== null) panelCommand('rename_group', { groupId: message.groupId, name: name });
        });
        return;

      case 'requestDeleteGroup':
        group = findGroup(message.groupId);
        if (!group) return;
        confirm(t('deleteGroup'), t('deleteGroupConfirm', group.name), t('delete'))
          .then(function (yes) {
            if (yes) panelCommand('delete_group', { groupId: message.groupId });
          });
        return;

      case 'requestDeleteServer':
        var doomed = findServer(message.groupId, message.serverId);
        if (!doomed) return;
        confirm(t('delete'), t('deleteServerConfirm', nameOf(doomed)), t('delete'))
          .then(function (yes) {
            if (yes) {
              panelCommand('delete_server', {
                groupId: message.groupId,
                serverId: message.serverId
              });
            }
          });
        return;

      case 'openConfig':
        invoke('open_config').catch(panelError);
        return;

      case 'openMemory':
        // What the assistant remembers arrives with stage 4. The row menu hides
        // this entry until then -- `aiEnabled` is false -- so reaching it means
        // something is out of step, and saying so beats doing nothing.
        console.info('[shell] assistant memory arrives with stage 4:', message.serverId);
        return;
    }

    console.info('[shell] servers pane, unhandled:', message.type, message);
  }

  function fromTerminal(pane, message) {
    if (message.type === 'ready') { connect(pane); return; }

    if (message.type === 'input') {
      var data = message.data || '';
      if (pane.connected) {
        invoke('terminal_input', {
          pane: pane.id,
          groupId: pane.groupId,
          serverId: pane.server.id,
          data: data
        }).then(function (accepted) {
          // The session went away between the keystroke and its arrival. Say so
          // once, here, rather than letting the next dozen keys vanish silently.
          if (!accepted && pane.connected) {
            pane.connected = false;
            note(pane, t('connectionClosedRetryEnter'));
          }
        }).catch(function () { pane.connected = false; });
        return;
      }
      // Enter on a dead session reconnects, exactly as it did under the
      // extension. Anything else is swallowed: there is nothing to type into.
      if (data.indexOf('\r') >= 0 || data.indexOf('\n') >= 0) connect(pane);
      return;
    }

    if (message.type === 'resize') {
      var cols = message.cols || 0;
      var rows = message.rows || 0;
      if (cols < 1 || rows < 1) return;

      /*
       * Two filters, and the terminal is unreadable without both.
       *
       * A hidden pane is `display: none`, so xterm's fit addon measures a box
       * with no width, gives up, and leaves the old size in place -- but
       * terminal.js reports whatever `term.cols` says regardless of whether the
       * fit changed anything. So every tab switch produced a resize message
       * carrying the size the pty already had.
       *
       * Forwarding one is not free. A window_change is a SIGWINCH even when the
       * numbers are identical, and readline answers a SIGWINCH by redrawing its
       * prompt. That redraw was the half-written `[trade@local` line appearing
       * on every switch -- twice, in fact, because scheduleResize fires once on
       * the next frame and again 50ms later.
       */
      if (!showing(pane)) return;

      // Remembered even while disconnected, so the next connection opens its
      // pty at the size the window is already showing.
      pane.cols = cols;
      pane.rows = rows;
      paintStatus();

      /*
       * Compared against what the pty was last *told*, not against what the page
       * last said. Those differ: a resize that lands before the connection is
       * up updates the pane and goes no further, and comparing against the pane
       * would then treat the identical message that follows -- the one that
       * could finally be sent -- as a no-op. The pty stayed at the 120x36 it was
       * opened with while xterm drew about 68 columns, and readline, doing its
       * cursor arithmetic in 120, wrapped in all the wrong places. That is the
       * mangled prompt, and it survived every tab switch because every later
       * resize was deduplicated away too.
       */
      if (!pane.connected) return;
      if (cols === pane.ptyCols && rows === pane.ptyRows) return;
      pane.ptyCols = cols;
      pane.ptyRows = rows;
      invoke('terminal_resize', { pane: pane.id, cols: cols, rows: rows });
      return;
    }

    if (message.type === 'openTransfer') {
      if (pane.server) openTransfer(pane.server, pane.groupId, pane);
      return;
    }
    if (message.type === 'openAi') { openChat(pane); return; }
    if (message.type === 'copyText') {
      navigator.clipboard.writeText(message.text || '').catch(function () {});
      return;
    }
    if (message.type === 'readClipboard') {
      navigator.clipboard.readText().then(function (text) {
        send(pane.id, { type: 'clipboardText', text: text });
      }).catch(function () {});
      return;
    }
    console.info('[shell] terminal pane, not wired yet:', message.type, message);
  }

  window.addEventListener('message', function (event) {
    var data = event.data;
    if (!data || data.__tshell !== 'page') return;
    var message = data.payload || {};

    /*
     * Raised by every page's bridge on a click inside it, and by nothing else.
     * It settles which column the title bar acts on and where the next tab
     * opens, so it is answered before anything looks at what kind of pane sent
     * it -- the sidebar excepted, which is not in a column at all.
     */
    if (message.type === 'paneFocus') {
      if (data.paneId !== 'servers') focusPane(data.paneId);
      return;
    }

    if (data.paneId === 'servers') { fromServers(message); return; }

    var pane = panes.get(data.paneId);
    if (!pane) return;
    if (pane.kind === 'terminal') { fromTerminal(pane, message); return; }
    if (pane.kind === 'transfer') { fromTransfer(pane, message); return; }
    if (pane.kind === 'chat') { fromChat(pane, message); return; }
    if (pane.kind === 'settings') {
      if (message.type === 'setTheme') { applyTheme(message.theme); return; }
      /*
       * Two messages, not one, because a colour picker being dragged and a
       * colour picker being let go are different questions. `schemesApply`
       * repaints every terminal and touches nothing; `schemesSave` writes.
       */
      if (message.type === 'schemesApply') { applySchemes(message.file); return; }
      if (message.type === 'schemesSave') { saveSchemes(pane.id, message.file); return; }
      if (message.type === 'fontsRequest') { sendFonts(pane.id); return; }
      /*
       * A question the page wants asked, in the page's own words.
       *
       * The dialogs live here because an undecorated window has no `showWarning`
       * to borrow and the browser's own `confirm` freezes the whole WebView.
       * The wording comes from the page, because the settings page's strings are
       * the settings page's -- this frame has a table with no keys for them.
       */
      if (message.type === 'confirm') {
        confirm(message.title || '', message.message || '', message.accept || c('ok'))
          .then(function (ok) {
            send(pane.id, { type: 'confirmed', token: message.token, ok: ok });
          });
        return;
      }
      // The assistant's own settings. Read back from Rust rather than echoed,
      // because what it settled on after clamping is what is true.
      if (message.type === 'aiRead') {
        invoke('ai_settings_read').then(function (settings) {
          send(pane.id, { type: 'aiSettings', settings: settings });
        });
        return;
      }
      /*
       * The language is written and the window is rebuilt.
       *
       * Every page reads its labels out of the bootstrap it loaded with, so
       * there is nothing to send that would re-letter one. Reloading is the
       * honest answer and it is what the settings page's hint promises.
       */
      if (message.type === 'setLanguage') {
        invoke('set_language', { language: message.language })
          .then(releaseSessions)
          .then(function () { window.location.reload(); })
          .catch(function (error) { console.error('[shell] the language would not save:', error); });
        return;
      }
      if (message.type === 'aiWrite') {
        invoke('ai_settings_write', { patch: message.patch }).then(function (settings) {
          send(pane.id, { type: 'aiSettings', settings: settings });
          // The toolbar of every open chat panel follows: these are global.
          broadcastAiState();
          // Switching the assistant off has to reach the terminal's own button
          // and the server panel, neither of which is a chat pane.
          refreshState();
        });
        return;
      }
      return;
    }
    console.info('[shell] ' + pane.kind + ' pane, not wired yet:', message.type, message);
  });

  // ------------------------------------------------------------ panel state ---

  /*
   * The last state Rust handed back, and the only copy of it.
   *
   * Every command that changes the config answers with the state that followed
   * it, so there is nothing to patch here and nothing that can drift: the panel
   * shows the file, or it shows the error that stopped it from being read.
   */
  var panel = {
    groups: [],
    passwords: {},
    privateKeyPassphrases: {},
    aiEnabled: false,
    secrets: { backend: 'keychain', reason: null }
  };

  function sidebarFrame() {
    var frame = sidebar.querySelector('iframe');
    return frame && frame.contentWindow ? frame : null;
  }

  function toSidebar(payload) {
    var frame = sidebarFrame();
    if (frame) frame.contentWindow.postMessage({ __tshell: 'host', payload: payload }, '*');
  }

  function pushState() {
    toSidebar({
      type: 'state',
      groups: panel.groups,
      passwords: panel.passwords,
      privateKeyPassphrases: panel.privateKeyPassphrases,
      aiEnabled: panel.aiEnabled,
      /*
       * The table rides along with every state, not only the first. This is the
       * panel the language setting is changed from, and the change arrives as a
       * state update: sending the strings with it is what re-labels the tree
       * without the user having to close and reopen anything.
       */
      strings: pageStrings()
    });
  }

  function applyState(next) {
    panel = next;
    pushState();
  }

  /*
   * Rust rejects validation failures with the string table's own key --
   * `hostUserRequired`, `privateKeyRequired` -- so the message is translated
   * here rather than duplicated on both sides. `t` returns anything it does not
   * recognise unchanged, which is what carries a real error message through.
   */
  function panelError(error) {
    toSidebar({ type: 'error', message: t(String(error && error.message || error)) });
  }

  /** Every mutating panel command answers with the state that followed it. */
  function panelCommand(command, args) {
    return invoke(command, args || {}).then(applyState).catch(panelError);
  }

  function findGroup(groupId) {
    return panel.groups.filter(function (group) { return group.id === groupId; })[0] || null;
  }

  function findServer(groupId, serverId) {
    var group = findGroup(groupId);
    if (!group) return null;
    return group.servers.filter(function (s) { return s.id === serverId; })[0] || null;
  }

  function nameOf(server) {
    return server.name || server.host;
  }

  /*
   * Carry an edit to the server through to the tabs already open on it.
   *
   * A tab holds the server it was opened with, and that copy goes stale the
   * moment the panel saves. Encoding is the one field a running session can act
   * on straight away -- the decoder is swapped in place rather than the session
   * being reconnected, because reconnecting to change how bytes are read would
   * cost the scrollback and whatever was half-typed at the prompt.
   *
   * Host, port and credentials are not applied to a live session: those describe
   * a connection that already exists, and changing them means the next one.
   */
  function adoptEdits() {
    panes.forEach(function (pane) {
      if (!pane.server) return;
      var fresh = findServer(pane.groupId, pane.server.id);
      if (!fresh) return;
      var was = pane.server.encoding;
      pane.server = fresh;
      if (pane.kind === 'terminal' && pane.connected && fresh.encoding !== was) {
        invoke('terminal_reload_encoding', {
          pane: pane.id,
          groupId: pane.groupId,
          serverId: fresh.id
        });
      }
    });
  }

  function mountSidebar(afterLoad) {
    var iframe = document.createElement('iframe');
    iframe.src = frameUrl('servers/servers.html', 'servers', {
      strings: pageStrings(),
      language: language
    });
    // The page has to exist before it can be told anything, and the fragment
    // only carries the strings -- the tree itself arrives as a state message.
    iframe.onload = function () {
      pushState();
      if (afterLoad) afterLoad();
    };
    sidebar.innerHTML = '';
    sidebar.appendChild(iframe);
  }

  // ----------------------------------------------------------------- chrome ---

  /*
   * The title bar's own labels.
   *
   * vendor/strings.json was exported from the extension's i18n table, and the
   * extension had no window frame to name -- no minimise, no close, no panel to
   * hide. Those words live here until stage 1 ports i18n to Rust and they can
   * join the rest. The two that the extension did have (`agentTitle`,
   * `fileTransfer`) are read from the real table, not repeated here.
   */
  var chromeText = {
    'zh-CN': {
      settings: '设置',
      showSidebar: '显示服务器面板',
      hideSidebar: '隐藏服务器面板',
      minimize: '最小化',
      maximize: '最大化',
      restore: '向下还原',
      close: '关闭',
      splitLeft: '向左拆分',
      splitRight: '向右拆分',
      closeTab: '关闭',
      closeOthers: '关闭其它标签页',
      ok: '确定',
      stConnected: '已连接',
      stConnecting: '连接中',
      stOffline: '未连接',
      authFailed: '认证失败：服务器拒绝了这个用户名或密码。'
    },
    'en-US': {
      settings: 'Settings',
      showSidebar: 'Show server panel',
      hideSidebar: 'Hide server panel',
      minimize: 'Minimize',
      maximize: 'Maximize',
      restore: 'Restore down',
      close: 'Close',
      splitLeft: 'Split Left',
      splitRight: 'Split Right',
      closeTab: 'Close',
      closeOthers: 'Close Other Tabs',
      ok: 'OK',
      stConnected: 'Connected',
      stConnecting: 'Connecting',
      stOffline: 'Not connected',
      authFailed: 'Authentication failed: the server rejected this user or password.'
    }
  };

  function c(key) {
    return (chromeText[language] || chromeText['en-US'])[key];
  }

  /** Tooltip and screen-reader name are the same words; set them together. */
  function label(element, words) {
    element.title = words;
    element.setAttribute('aria-label', words);
  }

  var maximized = false;
  var sidebarHidden = false;

  /*
   * The three buttons an undecorated window has to provide for itself. Dragging
   * and double-click-to-maximise come from `data-tauri-drag-region` on the bar,
   * so the only state to track here is whether the window is maximised -- and it
   * is tracked off the resize event rather than off the click, because the window
   * can also be maximised by the drag region or by the OS.
   */
  var winMax = document.getElementById('win-max');

  function paintWindowButtons() {
    document.body.classList.toggle('maximized', maximized);
    label(winMax, maximized ? c('restore') : c('maximize'));
  }

  (function windowControls() {
    var api = window.__TAURI__ && window.__TAURI__.window;
    if (!api) return;
    var appWindow = api.getCurrentWindow();

    function syncMaximized() {
      appWindow.isMaximized()
        .then(function (on) { maximized = on; paintWindowButtons(); })
        .catch(function () {});
    }

    document.getElementById('win-min').onclick = function () { appWindow.minimize(); };
    winMax.onclick = function () { appWindow.toggleMaximize(); };
    document.getElementById('win-close').onclick = function () { appWindow.close(); };

    appWindow.onResized(syncMaximized);
    syncMaximized();
  })();

  document.getElementById('settings').onclick = openSettings;

  /** Every label on the bar, in whatever language boot settled on. */
  function paintChrome() {
    label(document.getElementById('settings'), c('settings'));
    label(document.getElementById('win-min'), c('minimize'));
    label(document.getElementById('win-close'), c('close'));
    label(actionAi, t('agentTitle'));
    label(actionTransfer, t('fileTransfer'));
    paintWindowButtons();
    paintSidebar();
  }

  actionAi.onclick = function () {
    var pane = frontTerminal();
    if (pane) openChat(pane);
  };

  actionTransfer.onclick = function () {
    var pane = frontTerminal();
    if (pane) openTransfer(pane.server, pane.groupId, pane);
  };

  /*
   * Hiding the server panel is a button and nothing else. The obvious shortcut
   * for it would be Ctrl+B, and that is exactly the key this application must
   * not take: it is tmux's prefix and readline's backward-char, and every one of
   * those keystrokes belongs to the terminal the user is typing into. Picking a
   * shortcut that no shell wants is a decision worth making deliberately, later.
   */
  var sidebarButton = document.getElementById('toggle-sidebar');
  var sidebarGlyph = document.getElementById('sidebar-glyph');

  function paintSidebar() {
    sidebar.hidden = sidebarHidden;
    document.getElementById('splitter').hidden = sidebarHidden;
    sidebarButton.classList.toggle('off', sidebarHidden);
    /*
     * The rail in the glyph empties when the panel is away. Two icons and a
     * swapped href rather than one icon with a hidden part: the glyph arrives
     * through a `<use>`, and the only thing document CSS reaches into the shadow
     * tree that builds is inherited properties -- `display` is not one, so the
     * rule that used to hide the dashes would silently do nothing.
     *
     * It also means the state survives being unable to tell the colours apart,
     * which dimming alone did not.
     */
    sidebarGlyph.setAttribute('href', sidebarHidden ? '#i-panel-off' : '#i-panel');
    label(sidebarButton, sidebarHidden ? c('showSidebar') : c('hideSidebar'));
  }

  (function sidebarToggle() {
    try {
      sidebarHidden = localStorage.getItem('tshell:sidebar') === 'hidden';
    } catch (error) {
      // The default stands.
    }

    sidebarButton.onclick = function () {
      sidebarHidden = !sidebarHidden;
      paintSidebar();
      try {
        localStorage.setItem('tshell:sidebar', sidebarHidden ? 'hidden' : 'shown');
      } catch (error) {
        // Remembering the choice is a convenience, not a requirement.
      }
    };

    paintSidebar();
  })();

  (function splitter() {
    var bar = document.getElementById('splitter');
    var dragging = false;
    bar.addEventListener('mousedown', function (event) {
      dragging = true;
      bar.classList.add('dragging');
      event.preventDefault();
      // Panes are iframes; without this they swallow the drag.
      panes.forEach(function (pane) { pane.iframe.style.pointerEvents = 'none'; });
      var frames = sidebar.querySelector('iframe');
      if (frames) frames.style.pointerEvents = 'none';
    });
    window.addEventListener('mousemove', function (event) {
      if (!dragging) return;
      var width = Math.min(Math.max(event.clientX, 180), window.innerWidth - 320);
      sidebar.style.width = width + 'px';
    });
    window.addEventListener('mouseup', function () {
      if (!dragging) return;
      dragging = false;
      bar.classList.remove('dragging');
      panes.forEach(function (pane) { pane.iframe.style.pointerEvents = ''; });
      var frames = sidebar.querySelector('iframe');
      if (frames) frames.style.pointerEvents = '';
    });
  })();

  // ------------------------------------------------------------------- boot ---

  fetch('vendor/strings.json')
    .then(function (response) { return response.json(); })
    .then(function (table) {
      strings = table;
      return invoke('app_info').catch(function (error) {
        console.warn('[shell] app_info unavailable:', error);
        return { name: 'tshell', version: 'dev', platform: 'browser', language: 'zh-CN' };
      });
    })
    .then(function (info) {
      language = strings[info.language] ? info.language : 'en-US';
      /*
       * The version, and nothing else. The platform and the language were on
       * this strip too, and neither is news: you know which machine you are
       * sitting at, and the language is legible from every other word on the
       * screen. Both are still in Settings, where a fact you go looking for
       * belongs.
       */
      document.getElementById('build').textContent = info.version;
      // Only now is there a table to name anything from.
      paintChrome();
      /*
       * Before anything can be opened, because a terminal that opened in one
       * scheme and switched to another a moment later would be doing it in
       * front of the user. Nothing has a pane yet, so there is no flash to
       * avoid -- only an order to keep.
       *
       * A file that will not parse is not a failure to boot, for the same
       * reason a config that will not parse is not: the built-in schemes are
       * enough to paint a terminal with. What is remembered is that it did not
       * parse, so that nothing is later written over it.
       */
      return invoke('schemes_load')
        .then(function (schemes) {
          window.tshellSchemes.load(schemes);
        })
        .catch(function (error) {
          schemesError = String(error);
          console.warn('[shell] the schemes file would not load:', error);
        })
        .then(function () { return invoke('load_state'); });
    })
    .then(function (loaded) {
      panel = loaded;
      aiEnabled = loaded.aiEnabled !== false;
      actionAi.hidden = !aiEnabled;
      mountSidebar();
    })
    .catch(function (error) {
      /*
       * A config that will not load is not a failure to boot. The window still
       * opens, the panel still mounts -- empty -- and the reason is put in front
       * of the user, because the file is theirs to fix and it has been left
       * exactly as it was. Failing dark here would leave them with no way in and
       * nothing said about why.
       */
      console.error(error);
      mountSidebar(function () { panelError(error); });
    });
})();
