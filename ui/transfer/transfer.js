(function () {
  'use strict';

  const vscode = acquireVsCodeApi();
  const boot = window.__tshell || {};
  const S = boot.strings || {};
  const ROW_H = 26; // Must match --row-h in transfer.css.
  const OVERSCAN = 8;
  const SVG_NS = 'http://www.w3.org/2000/svg';

  // Bumped whenever this file changes. Run `__tshellTransferBuild` in the
  // webview devtools console to see which version is actually loaded, instead
  // of guessing whether an edit took effect.
  window.__tshellTransferBuild = 9;

  const $ = (id) => document.getElementById(id);
  const post = (type, payload) => vscode.postMessage(Object.assign({ type: type }, payload || {}));
  const fmt = (template, value) => String(template || '').replace('{0}', String(value));

  // Highlighting lives in media/shared so the chat panel's code blocks and this
  // preview cannot drift apart. Bound to locals to keep the call sites unchanged.
  const escapeHtml = window.tshellHighlight.escapeHtml;
  const highlightLine = window.tshellHighlight.highlightLine;

  const COL_DEFAULTS = { size: 86, mod: 148 };
  const COL_MIN = 44;
  const COL_MAX = 320;

  let state = Object.assign(
    { split: 0, logHeight: 0, sort: {}, hidden: {}, cols: {}, fontSize: 12 },
    vscode.getState() || {}
  );
  if (!state.cols) state.cols = {};
  const saveState = () => vscode.setState(state);

  // -- formatting ---------------------------------------------------------

  function formatSize(size) {
    const value = Number(size) || 0;
    if (value < 1024) return value + ' B';
    if (value < 1048576) return (value / 1024).toFixed(1) + ' KB';
    if (value < 1073741824) return (value / 1048576).toFixed(1) + ' MB';
    return (value / 1073741824).toFixed(1) + ' GB';
  }

  function formatDuration(seconds) {
    if (!isFinite(seconds) || seconds < 0) return '--';
    const total = Math.round(seconds);
    if (total < 60) return total + 's';
    const minutes = Math.floor(total / 60);
    if (minutes < 60) return minutes + 'm' + String(total % 60).padStart(2, '0') + 's';
    return Math.floor(minutes / 60) + 'h' + String(minutes % 60).padStart(2, '0') + 'm';
  }

  function formatStamp(ms) {
    return ms ? new Date(ms).toLocaleString() : '';
  }

  function svgIcon(id, className) {
    const svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('class', className || 'icon');
    svg.setAttribute('aria-hidden', 'true');
    const use = document.createElementNS(SVG_NS, 'use');
    use.setAttribute('href', id);
    svg.append(use);
    return svg;
  }

  /* File kinds worth telling apart at a glance. Anything else stays neutral. */
  const ICON_KINDS = [
    ['ic-code', ['c', 'cc', 'cpp', 'cxx', 'h', 'hpp', 'java', 'js', 'jsx', 'ts', 'tsx', 'py', 'sh', 'bash', 'go', 'rs', 'rb', 'php', 'vue', 'lua', 'pl']],
    ['ic-markup', ['html', 'htm', 'xml', 'css', 'scss', 'less', 'svg', 'vcxproj', 'sln']],
    ['ic-data', ['json', 'yml', 'yaml', 'ini', 'conf', 'cfg', 'properties', 'env', 'csv', 'dbf', 'sql', 'toml']],
    ['ic-archive', ['zip', 'tar', 'gz', 'tgz', 'bz2', 'xz', '7z', 'rar', 'jar', 'war']],
    ['ic-binary', ['so', 'dll', 'exe', 'bin', 'o', 'a', 'lib', 'pdb', 'obj', 'class', 'pyc']],
    ['ic-doc', ['md', 'txt', 'log', 'pdf', 'doc', 'docx', 'rtf']]
  ];

  const EXTENSION_KIND = new Map();
  ICON_KINDS.forEach(([kind, extensions]) => extensions.forEach((extension) => EXTENSION_KIND.set(extension, kind)));

  function iconKind(entry) {
    if (entry.type === 'directory') return 'ic-dir';
    const name = String(entry.name || '');
    const dot = name.lastIndexOf('.');
    if (dot <= 0) return 'ic-plain';
    return EXTENSION_KIND.get(name.slice(dot + 1).toLowerCase()) || 'ic-plain';
  }

  function applyStrings() {
    document.querySelectorAll('[data-i18n]').forEach((el) => { el.textContent = S[el.dataset.i18n] || ''; });
    document.querySelectorAll('[data-i18n-title]').forEach((el) => { el.title = S[el.dataset.i18nTitle] || ''; });
    document.querySelectorAll('[data-i18n-aria]').forEach((el) => { el.setAttribute('aria-label', S[el.dataset.i18nAria] || ''); });
  }

  // -- panes --------------------------------------------------------------

  function createPane(side) {
    return {
      side: side,
      path: '',
      canGoUp: false,
      entries: [],
      rows: [],
      selected: new Set(),
      anchor: 0,
      lead: 0,
      dropIndex: -1,
      sort: state.sort[side] || { key: 'name', dir: 1 },
      cols: Object.assign({}, COL_DEFAULTS, state.cols[side]),
      showHidden: side in state.hidden ? state.hidden[side] : Boolean(boot.showHiddenFiles),
      typeBuffer: '',
      typeTimer: null,
      el: {
        pane: $('pane-' + side),
        path: $('path-' + side),
        list: $('list-' + side),
        head: $('head-' + side),
        headClip: $('headclip-' + side),
        hidden: $('hidden-' + side),
        window: $('list-' + side).querySelector('.window'),
        sizer: $('list-' + side).querySelector('.sizer')
      }
    };
  }

  const panes = { local: createPane('local'), remote: createPane('remote') };
  let activeSide = 'local';

  function other(side) {
    return side === 'local' ? 'remote' : 'local';
  }

  function activate(side) {
    if (activeSide === side) return;
    const previous = panes[activeSide];
    activeSide = side;
    panes.local.el.pane.classList.toggle('active', side === 'local');
    panes.remote.el.pane.classList.toggle('active', side === 'remote');
    // Only one pane holds a selection at a time, so moving to the other side
    // drops what was selected here. A drag is unaffected: it starts from this
    // pane and drops on the other one without ever activating it.
    if (previous.selected.size) {
      previous.selected.clear();
      paintSelection(previous);
    }
  }

  function sortRows(pane) {
    const key = pane.sort.key;
    const dir = pane.sort.dir;
    const sorted = pane.entries.slice().sort((a, b) => {
      const aDir = a.type === 'directory';
      const bDir = b.type === 'directory';
      // Folders stay above files whichever column is sorted.
      if (aDir !== bDir) return aDir ? -1 : 1;
      let cmp = 0;
      if (key === 'size') cmp = (a.size || 0) - (b.size || 0);
      else if (key === 'modified') cmp = (a.modifiedAt || 0) - (b.modifiedAt || 0);
      else cmp = a.name.localeCompare(b.name);
      if (cmp === 0) cmp = a.name.localeCompare(b.name);
      return cmp * dir;
    });
    // Going up is the toolbar button, Backspace or Alt+Left -- there is no
    // pseudo entry taking up the first row.
    pane.rows = sorted;
    updateSortHeader(pane);
  }

  /* Column widths drive both the header and every row through one pair of vars. */
  function applyColumnWidths(pane) {
    pane.el.pane.style.setProperty('--w-size', pane.cols.size + 'px');
    pane.el.pane.style.setProperty('--w-mod', pane.cols.mod + 'px');
  }

  function updateSortHeader(pane) {
    pane.el.head.querySelectorAll('.sortable').forEach((el) => {
      const active = el.dataset.key === pane.sort.key;
      el.classList.toggle('sorted', active);
      el.classList.toggle('desc', active && pane.sort.dir < 0);
    });
  }

  function rowElement(pane, entry, index) {
    const row = document.createElement('div');
    row.className = 'row' + (entry.type === 'directory' ? ' dir' : '');
    row.dataset.index = String(index);
    row.setAttribute('role', 'option');
    const selected = pane.selected.has(entry.path);
    row.setAttribute('aria-selected', selected ? 'true' : 'false');
    if (selected) row.classList.add('selected');
    if (index === pane.lead && pane.selected.size) row.classList.add('lead');
    if (index === pane.dropIndex) row.classList.add('drop-target');
    row.draggable = true;

    const iconId = entry.type === 'directory' ? '#i-folder'
      : entry.type === 'symlink' ? '#i-link' : '#i-file';
    const iconClass = 'icon ' + iconKind(entry);
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = entry.name;
    name.title = entry.path;
    const size = document.createElement('span');
    size.className = 'num';
    size.textContent = entry.type === 'directory' ? '' : formatSize(entry.size);
    const modified = document.createElement('span');
    modified.textContent = formatStamp(entry.modifiedAt);

    row.append(svgIcon(iconId, iconClass), name, size, modified);
    return row;
  }

  /** Renders only the rows in view, so a folder with 50k entries still scrolls. */
  function render(pane) {
    const list = pane.el.list;
    const total = pane.rows.length;
    pane.el.sizer.style.height = (total * ROW_H) + 'px';
    pane.el.window.textContent = '';

    if (total === 0) {
      pane.el.window.style.transform = 'translateY(0)';
      const empty = document.createElement('div');
      empty.className = 'pane-empty';
      empty.textContent = S.emptyFolder || '';
      pane.el.window.append(empty);
      return;
    }

    const start = Math.max(0, Math.floor(list.scrollTop / ROW_H) - OVERSCAN);
    const end = Math.min(total, start + Math.ceil(list.clientHeight / ROW_H) + OVERSCAN * 2);
    pane.el.window.style.transform = 'translateY(' + (start * ROW_H) + 'px)';
    const fragment = document.createDocumentFragment();
    for (let index = start; index < end; index += 1) fragment.append(rowElement(pane, pane.rows[index], index));
    pane.el.window.append(fragment);
  }

  /**
   * Repaints selection state on the rows already in the DOM.
   *
   * Selection changes must never rebuild the rows: a mousedown handler that
   * replaces its own target leaves the browser with nothing to dispatch the
   * following click or dblclick on, which would break "double-click to open"
   * and abort drags mid-gesture.
   */
  function paintSelection(pane) {
    pane.el.window.querySelectorAll('.row').forEach((row) => {
      const index = Number(row.dataset.index);
      const entry = pane.rows[index];
      if (!entry) return;
      const selected = pane.selected.has(entry.path);
      row.classList.toggle('selected', selected);
      row.classList.toggle('lead', index === pane.lead && pane.selected.size > 0);
      row.classList.toggle('drop-target', index === pane.dropIndex);
      row.setAttribute('aria-selected', selected ? 'true' : 'false');
    });
  }

  function ensureVisible(pane, index) {
    const list = pane.el.list;
    const top = index * ROW_H;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + ROW_H > list.scrollTop + list.clientHeight) list.scrollTop = top + ROW_H - list.clientHeight;
  }

  // -- selection ----------------------------------------------------------

  function selectSingle(pane, index) {
    pane.lead = index;
    pane.anchor = index;
    pane.selected.clear();
    const entry = pane.rows[index];
    if (entry) pane.selected.add(entry.path);
  }

  function toggleAt(pane, index) {
    pane.lead = index;
    pane.anchor = index;
    const entry = pane.rows[index];
    if (!entry) return;
    if (pane.selected.has(entry.path)) pane.selected.delete(entry.path);
    else pane.selected.add(entry.path);
  }

  function selectRange(pane, to, additive) {
    if (!additive) pane.selected.clear();
    const start = Math.min(pane.anchor, to);
    const end = Math.max(pane.anchor, to);
    for (let index = start; index <= end; index += 1) {
      const entry = pane.rows[index];
      if (entry) pane.selected.add(entry.path);
    }
    pane.lead = to;
  }

  function selectAll(pane) {
    pane.selected.clear();
    pane.rows.forEach((entry) => pane.selected.add(entry.path));
  }

  function selectedItems(pane) {
    return pane.rows
      .filter((entry) => pane.selected.has(entry.path))
      .map((entry) => ({ path: entry.path, isDirectory: entry.type === 'directory' }));
  }

  function clampIndex(pane, index) {
    return Math.max(0, Math.min(pane.rows.length - 1, index));
  }

  function moveLead(pane, index, event) {
    const next = clampIndex(pane, index);
    if (event.shiftKey) selectRange(pane, next, false);
    else if (event.ctrlKey || event.metaKey) pane.lead = next;
    else selectSingle(pane, next);
    ensureVisible(pane, next);
    render(pane);
  }

  // -- navigation ---------------------------------------------------------

  function openIndex(pane, index) {
    const entry = pane.rows[index];
    if (!entry) return;
    if (entry.type === 'directory') { post('list', { side: pane.side, path: entry.path }); return; }
    openPreview(pane.side, entry.path);
  }

  function goUp(pane) {
    if (pane.canGoUp) post('up', { side: pane.side });
  }

  function transferSelection(side, targetDir) {
    const items = selectedItems(panes[side]);
    post('transfer', { side: side, items: items, targetDir: targetDir });
  }

  // -- type-ahead ---------------------------------------------------------

  function handleTypeahead(pane, key) {
    if (pane.typeTimer) clearTimeout(pane.typeTimer);
    pane.typeTimer = setTimeout(() => { pane.typeBuffer = ''; }, 800);
    const lower = key.toLowerCase();
    // Repeating one letter cycles through the entries starting with it.
    const repeated = pane.typeBuffer.length > 0
      && pane.typeBuffer.split('').every((char) => char === pane.typeBuffer[0])
      && lower === pane.typeBuffer[0].toLowerCase();
    let prefix;
    let startIndex = 0;
    if (repeated) {
      prefix = lower;
      startIndex = pane.lead + 1;
      pane.typeBuffer += key;
    } else {
      pane.typeBuffer += key;
      prefix = pane.typeBuffer.toLowerCase();
    }
    const total = pane.rows.length;
    for (let offset = 0; offset < total; offset += 1) {
      const index = (startIndex + offset) % total;
      const entry = pane.rows[index];
      if (entry && entry.name.toLowerCase().startsWith(prefix)) {
        selectSingle(pane, index);
        ensureVisible(pane, index);
        render(pane);
        return;
      }
    }
  }

  // -- pane wiring --------------------------------------------------------

  function wirePane(pane) {
    const list = pane.el.list;

    pane.el.path.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') post('list', { side: pane.side, path: pane.el.path.value });
      if (event.key === 'Escape') { pane.el.path.value = pane.path; list.focus(); }
    });
    pane.el.path.addEventListener('focus', () => activate(pane.side));
    $('up-' + pane.side).onclick = () => goUp(pane);
    $('refresh-' + pane.side).onclick = () => post('refresh', { side: pane.side });
    pane.el.hidden.onclick = () => {
      pane.showHidden = !pane.showHidden;
      pane.el.hidden.classList.toggle('on', pane.showHidden);
      pane.el.hidden.setAttribute('aria-pressed', pane.showHidden ? 'true' : 'false');
      state.hidden[pane.side] = pane.showHidden;
      saveState();
      post('setHidden', { side: pane.side, value: pane.showHidden });
    };
    pane.el.hidden.classList.toggle('on', pane.showHidden);
    pane.el.hidden.setAttribute('aria-pressed', pane.showHidden ? 'true' : 'false');

    pane.el.head.querySelectorAll('.sortable').forEach((el) => {
      el.onclick = () => {
        const key = el.dataset.key;
        pane.sort = pane.sort.key === key ? { key: key, dir: -pane.sort.dir } : { key: key, dir: 1 };
        state.sort[pane.side] = pane.sort;
        saveState();
        sortRows(pane);
        render(pane);
      };
    });

    // Dragging a grip left widens the column to its right; the name column,
    // being the flexible one, absorbs the difference.
    pane.el.head.querySelectorAll('.grip').forEach((grip) => {
      grip.addEventListener('mousedown', (event) => {
        event.preventDefault();
        event.stopPropagation();
        columnDrag = { pane: pane, key: grip.dataset.col, startX: event.clientX, startWidth: pane.cols[grip.dataset.col] };
      });
      // The grip sits on top of the header cell, which sorts on click.
      grip.addEventListener('click', (event) => event.stopPropagation());
    });

    let scrollPending = false;
    list.addEventListener('scroll', () => {
      /*
       * Sideways first, and not inside the frame below.
       *
       * The headings are in a box of their own so that the list's vertical bar
       * can sit against the pane's edge rather than against the right-hand end
       * of a row (see the note in transfer.css). The price is that they no
       * longer move by themselves, and a heading that lags a frame behind the
       * column it names is worse than no heading: this assignment is cheap and
       * belongs in the same frame as the scroll that caused it.
       */
      pane.el.headClip.scrollLeft = list.scrollLeft;
      if (scrollPending) return;
      scrollPending = true;
      requestAnimationFrame(() => { scrollPending = false; render(pane); });
    });
    list.addEventListener('focus', () => activate(pane.side));

    list.addEventListener('mousedown', (event) => {
      activate(pane.side);
      // Right-click selection belongs to the contextmenu handler, which keeps an
      // existing multi-selection intact. Touching it here would collapse it.
      if (event.button === 2) return;
      const row = event.target.closest('.row');
      if (!row) {
        // Clicking the empty space below the entries clears the selection.
        if (pane.selected.size) { pane.selected.clear(); paintSelection(pane); }
        return;
      }
      const index = Number(row.dataset.index);
      if (event.shiftKey) selectRange(pane, index, event.ctrlKey || event.metaKey);
      else if (event.ctrlKey || event.metaKey) toggleAt(pane, index);
      else if (!pane.selected.has(pane.rows[index].path)) selectSingle(pane, index);
      else pane.lead = index;
      paintSelection(pane);
    });

    // A plain click on an already-selected row collapses the selection, but only
    // after mouseup so that dragging a multi-selection still works.
    list.addEventListener('click', (event) => {
      if (event.shiftKey || event.ctrlKey || event.metaKey) return;
      const row = event.target.closest('.row');
      if (!row) return;
      const index = Number(row.dataset.index);
      if (pane.selected.size > 1) { selectSingle(pane, index); paintSelection(pane); }
    });

    list.addEventListener('dblclick', (event) => {
      const row = event.target.closest('.row');
      if (row) openIndex(pane, Number(row.dataset.index));
    });

    list.addEventListener('keydown', (event) => onPaneKeyDown(pane, event));

    list.addEventListener('dragstart', (event) => {
      const row = event.target.closest('.row');
      if (!row) { event.preventDefault(); return; }
      const index = Number(row.dataset.index);
      const entry = pane.rows[index];
      if (!entry) { event.preventDefault(); return; }
      if (!pane.selected.has(entry.path)) { selectSingle(pane, index); paintSelection(pane); }
      dragFrom = pane.side;
      event.dataTransfer.effectAllowed = 'copy';
      event.dataTransfer.setData('text/plain', selectedItems(pane).map((item) => item.path).join('\n'));
      $('panes').classList.add('dragging');
    });

    list.addEventListener('dragover', (event) => {
      if (!dragFrom || dragFrom === pane.side) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = 'copy';
      const row = event.target.closest('.row');
      const index = row ? Number(row.dataset.index) : -1;
      const entry = index >= 0 ? pane.rows[index] : null;
      // Dropping onto a folder row targets that folder instead of the open one.
      const next = entry && entry.type === 'directory' ? index : -1;
      if (next !== pane.dropIndex) { pane.dropIndex = next; paintSelection(pane); }
    });

    list.addEventListener('dragleave', (event) => {
      if (list.contains(event.relatedTarget)) return;
      if (pane.dropIndex !== -1) { pane.dropIndex = -1; paintSelection(pane); }
    });

    list.addEventListener('drop', (event) => {
      event.preventDefault();
      if (!dragFrom || dragFrom === pane.side) { endDrag(); return; }
      const targetDir = pane.dropIndex >= 0 ? pane.rows[pane.dropIndex].path : pane.path;
      transferSelection(dragFrom, targetDir);
      endDrag();
    });
  }

  function onPaneKeyDown(pane, event) {
    const key = event.key;
    const pageStep = Math.max(1, Math.floor(pane.el.list.clientHeight / ROW_H) - 1);

    if (key === 'Tab') { event.preventDefault(); panes[other(pane.side)].el.list.focus(); return; }
    if (key === 'ArrowDown') { event.preventDefault(); moveLead(pane, pane.lead + 1, event); return; }
    if (key === 'ArrowUp') { event.preventDefault(); moveLead(pane, pane.lead - 1, event); return; }
    if (key === 'PageDown') { event.preventDefault(); moveLead(pane, pane.lead + pageStep, event); return; }
    if (key === 'PageUp') { event.preventDefault(); moveLead(pane, pane.lead - pageStep, event); return; }
    if (key === 'Home') { event.preventDefault(); moveLead(pane, 0, event); return; }
    if (key === 'End') { event.preventDefault(); moveLead(pane, pane.rows.length - 1, event); return; }
    if (key === 'Enter') { event.preventDefault(); openIndex(pane, pane.lead); return; }
    if (key === 'Backspace') { event.preventDefault(); goUp(pane); return; }
    if (key === 'F5') { event.preventDefault(); post('refresh', { side: pane.side }); return; }
    if (key === 'Escape') { event.preventDefault(); pane.el.path.focus(); pane.el.path.select(); return; }
    if (key === ' ' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); toggleAt(pane, pane.lead); paintSelection(pane); return; }
    if (event.ctrlKey || event.altKey || event.metaKey) return;
    if (key.length === 1 && (key !== ' ' || pane.typeBuffer)) {
      event.preventDefault();
      handleTypeahead(pane, key);
    }
  }

  let dragFrom = null;

  function endDrag() {
    dragFrom = null;
    $('panes').classList.remove('dragging');
    [panes.local, panes.remote].forEach((pane) => {
      if (pane.dropIndex !== -1) { pane.dropIndex = -1; paintSelection(pane); }
    });
  }

  window.addEventListener('dragend', endDrag);

  // -- context menus ------------------------------------------------------

  const paneMenu = $('paneMenu');
  const logMenu = $('logMenu');
  let menuSide = 'local';

  function hideMenus() {
    paneMenu.classList.remove('open');
    logMenu.classList.remove('open');
  }

  function placeMenu(menu, x, y) {
    menu.classList.add('open');
    const rect = menu.getBoundingClientRect();
    menu.style.left = Math.min(x, window.innerWidth - rect.width - 4) + 'px';
    menu.style.top = Math.min(y, window.innerHeight - rect.height - 4) + 'px';
  }

  document.addEventListener('contextmenu', (event) => {
    const target = event.target;
    if (target instanceof HTMLInputElement) { hideMenus(); return; }
    event.preventDefault();
    hideMenus();

    if ($('log').contains(target)) { placeMenu(logMenu, event.clientX, event.clientY); return; }

    for (const side of ['local', 'remote']) {
      const pane = panes[side];
      if (!pane.el.pane.contains(target)) continue;
      activate(side);
      menuSide = side;
      const row = target.closest ? target.closest('.row') : null;
      if (row) {
        const index = Number(row.dataset.index);
        const entry = pane.rows[index];
        // Right-clicking outside the current selection moves it, as in Explorer.
        if (entry && !pane.selected.has(entry.path)) { selectSingle(pane, index); paintSelection(pane); }
      }
      const uploading = side === 'local';
      const count = pane.selected.size;
      $('menuTransferText').textContent = uploading ? S.upload : S.download;
      $('menuTransferIcon').querySelector('use').setAttribute('href', uploading ? '#i-upload' : '#i-download');
      $('menuTransfer').style.display = count ? '' : 'none';
      $('menuSepTransfer').style.display = count ? '' : 'none';
      // Renaming is only meaningful for exactly one entry.
      $('menuRename').style.display = count === 1 ? '' : 'none';
      $('menuDelete').style.display = count ? '' : 'none';
      placeMenu(paneMenu, event.clientX, event.clientY);
      return;
    }
  });

  $('menuTransfer').onclick = () => { transferSelection(menuSide); hideMenus(); };
  $('menuRefresh').onclick = () => { post('refresh', { side: menuSide }); hideMenus(); };
  $('menuNewFolder').onclick = () => { post('newFolder', { side: menuSide }); hideMenus(); };
  $('menuRename').onclick = () => {
    const items = selectedItems(panes[menuSide]);
    if (items.length === 1) post('renameEntry', { side: menuSide, path: items[0].path });
    hideMenus();
  };
  $('menuDelete').onclick = () => {
    post('deleteEntries', { side: menuSide, items: selectedItems(panes[menuSide]) });
    hideMenus();
  };
  $('menuCopyPath').onclick = () => {
    const pane = panes[menuSide];
    const paths = selectedItems(pane).map((item) => item.path);
    post('copyText', { text: paths.length ? paths.join('\n') : pane.path });
    hideMenus();
  };
  $('menuCopyLog').onclick = () => {
    const selection = window.getSelection();
    const selected = selection && !selection.isCollapsed && $('log').contains(selection.getRangeAt(0).commonAncestorContainer)
      ? selection.toString()
      : '';
    const all = Array.from($('log').querySelectorAll('.log-line')).map((line) => line.dataset.raw || line.textContent).join('\n');
    post('copyText', { text: selected || all });
    hideMenus();
  };

  document.addEventListener('click', hideMenus);
  window.addEventListener('blur', hideMenus);

  // -- splitters ----------------------------------------------------------

  let resizing = null;
  let columnDrag = null;

  $('splitter').addEventListener('mousedown', (event) => { resizing = 'split'; event.preventDefault(); });
  $('logResizer').addEventListener('mousedown', (event) => { resizing = 'log'; event.preventDefault(); });
  window.addEventListener('mouseup', () => {
    if (columnDrag) {
      state.cols[columnDrag.pane.side] = columnDrag.pane.cols;
      saveState();
      columnDrag = null;
    }
    resizing = null;
  });
  window.addEventListener('mousemove', (event) => {
    if (columnDrag) {
      const delta = columnDrag.startX - event.clientX;
      columnDrag.pane.cols[columnDrag.key] = Math.max(COL_MIN, Math.min(COL_MAX, columnDrag.startWidth + delta));
      applyColumnWidths(columnDrag.pane);
      return;
    }
    if (!resizing) return;
    if (resizing === 'split') {
      const width = Math.max(180, Math.min(window.innerWidth - 187, event.clientX));
      state.split = width;
      $('panes').style.setProperty('--split', width + 'px');
    } else {
      const height = Math.max(80, Math.min(window.innerHeight - 160, window.innerHeight - event.clientY));
      state.logHeight = height;
      document.body.style.setProperty('--log-height', height + 'px');
    }
    saveState();
  });

  // -- progress -----------------------------------------------------------

  const progressState = { start: 0, lastTime: 0, lastBytes: 0, speed: 0 };

  function resetProgress() {
    progressState.start = 0;
    progressState.lastTime = 0;
    progressState.lastBytes = 0;
    progressState.speed = 0;
  }

  function showProgress(message) {
    const panel = $('transferPanel');
    panel.classList.add('open');
    const overall = message.overall || {};
    const current = message.current || {};

    if (message.phase === 'scanning') {
      // A previous single-file transfer may have hidden the overall row.
      panel.querySelector('.overall').style.display = '';
      panel.querySelector('.current').style.display = 'none';
      $('overallLabel').textContent = S.scanning || '';
      $('overallFill').style.width = '0%';
      $('overallMeta').textContent = fmt(S.filesProgress, overall.totalFiles || 0) + ' · ' + formatSize(overall.totalBytes || 0);
      resetProgress();
      return;
    }

    const now = Date.now();
    if (!progressState.start) {
      progressState.start = now;
      progressState.lastTime = now;
      progressState.lastBytes = overall.doneBytes || 0;
    }
    const delta = (now - progressState.lastTime) / 1000;
    if (delta >= 0.05) {
      const instant = Math.max(0, ((overall.doneBytes || 0) - progressState.lastBytes) / delta);
      progressState.speed = progressState.speed > 0 ? progressState.speed * 0.5 + instant * 0.5 : instant;
      progressState.lastTime = now;
      progressState.lastBytes = overall.doneBytes || 0;
    }
    const elapsed = (now - progressState.start) / 1000;
    const average = elapsed > 0 ? (overall.doneBytes || 0) / elapsed : 0;
    const speed = progressState.speed > 0 ? progressState.speed : average;
    const left = Math.max(0, (overall.totalBytes || 0) - (overall.doneBytes || 0));
    const eta = left > 0 && speed > 0 ? left / speed : (left === 0 ? 0 : NaN);

    // A single file needs one bar, not two saying the same thing.
    const multiple = (overall.totalFiles || 0) > 1;
    $('transferPanel').querySelector('.overall').style.display = multiple ? '' : 'none';
    $('transferPanel').querySelector('.current').style.display = '';

    if (multiple) {
      const ratio = overall.totalBytes > 0 ? Math.min(1, overall.doneBytes / overall.totalBytes) : 0;
      $('overallLabel').textContent = S.overallProgress + ' ' + (overall.doneFiles || 0) + '/' + (overall.totalFiles || 0);
      $('overallFill').style.width = Math.round(ratio * 100) + '%';
      $('overallMeta').textContent = formatSize(overall.doneBytes) + ' / ' + formatSize(overall.totalBytes)
        + ' · ' + Math.round(ratio * 100) + '%'
        + ' · ' + S.elapsed + ' ' + formatDuration(elapsed)
        + ' · ' + S.remaining + ' ' + formatDuration(eta);
    }

    const currentRatio = current.total > 0 ? Math.min(1, current.transferred / current.total) : 0;
    $('currentLabel').textContent = current.name || '';
    $('currentLabel').title = current.name || '';
    $('currentFill').style.width = Math.round(currentRatio * 100) + '%';
    const currentMeta = formatSize(current.transferred) + ' / ' + formatSize(current.total)
      + ' · ' + Math.round(currentRatio * 100) + '%'
      + ' · ' + formatSize(speed) + '/s'
      + (multiple ? '' : ' · ' + S.remaining + ' ' + formatDuration(eta));
    $('currentMeta').textContent = currentMeta + '  ';
  }

  function endProgress() {
    $('transferPanel').classList.remove('open');
    $('cancelTransfer').disabled = false;
    resetProgress();
  }

  $('cancelTransfer').onclick = () => {
    $('cancelTransfer').disabled = true;
    post('cancelTransfer');
  };

  // -- conflict dialog ----------------------------------------------------

  let conflictId = 0;

  function showConflict(message) {
    conflictId = message.id;
    $('conflictQuestion').textContent = fmt(S.conflictQuestion, message.name);
    const warning = $('conflictWarning');
    if (message.sourceKind !== message.targetKind) {
      warning.textContent = message.sourceKind === 'file' ? S.conflictFileOverDir : S.conflictDirOverFile;
      warning.style.display = '';
    } else {
      warning.style.display = 'none';
    }
    const describe = (kind, size, modifiedAt, targetPath) => {
      if (kind === 'directory') return targetPath;
      const stamp = modifiedAt ? ' · ' + formatStamp(modifiedAt) : '';
      return targetPath + '\n' + formatSize(size) + stamp;
    };
    $('conflictTargetDetail').textContent = describe(message.targetKind, message.targetSize, message.targetModifiedAt, message.targetPath);
    $('conflictSourceDetail').textContent = describe(message.sourceKind, message.sourceSize, message.sourceModifiedAt, message.sourcePath);
    $('conflictRemaining').textContent = message.remaining > 0 ? fmt(S.conflictRemaining, message.remaining) : '';
    $('conflictModal').classList.add('open');
    $('conflictOverwrite').focus();
  }

  function answerConflict(choice) {
    $('conflictModal').classList.remove('open');
    post('conflictResult', { id: conflictId, choice: choice });
  }

  $('conflictOverwrite').onclick = () => answerConflict('overwrite');
  $('conflictSkip').onclick = () => answerConflict('skip');
  $('conflictOverwriteAll').onclick = () => answerConflict('overwriteAll');
  $('conflictSkipAll').onclick = () => answerConflict('skipAll');
  $('conflictCancelAll').onclick = () => answerConflict('cancelAll');

  // -- preview ------------------------------------------------------------

  const preview = {
    side: 'remote',
    path: '',
    encoding: boot.defaultEncoding || 'utf8',
    content: '',
    language: 'text',
    done: true,
    nextOffset: 0,
    dbfNextRecord: 0,
    loading: false,
    pendingLine: '',
    renderedLines: 0,
    highlightState: window.tshellHighlight.newState(),
    tableMode: false
  };

  function openPreview(side, path) {
    preview.side = side;
    post('openPreview', { side: side, path: path, encoding: preview.encoding });
  }

  function parseCsv(content) {
    const rows = [];
    let row = [];
    let cell = '';
    let quoted = false;
    for (let index = 0; index < content.length; index += 1) {
      const char = content[index];
      if (quoted) {
        if (char === '"' && content[index + 1] === '"') { cell += '"'; index += 1; }
        else if (char === '"') quoted = false;
        else cell += char;
      } else if (char === '"') quoted = true;
      else if (char === ',') { row.push(cell); cell = ''; }
      else if (char === '\n') { row.push(cell); rows.push(row); row = []; cell = ''; }
      else if (char !== '\r') cell += char;
    }
    row.push(cell);
    if (row.length > 1 || row[0]) rows.push(row);
    return rows;
  }

  function renderTable(headers, rows, startIndex) {
    let html = '<table class="csv-table"><thead><tr><th class="row-index">#</th>';
    for (const header of headers) html += '<th>' + escapeHtml(header) + '</th>';
    html += '</tr></thead><tbody>';
    for (let rowIndex = 0; rowIndex < rows.length; rowIndex += 1) {
      const row = rows[rowIndex];
      html += '<tr><td class="row-index">' + (startIndex + rowIndex) + '</td>';
      for (let index = 0; index < Math.max(row.length, headers.length); index += 1) html += '<td>' + escapeHtml(row[index] || '') + '</td>';
      html += '</tr>';
    }
    return html + '</tbody></table>';
  }

  function renderCsvTable(content) {
    const rows = parseCsv(content);
    if (!rows.length) return '<div class="unsupported-preview"></div>';
    return renderTable(rows[0], rows.slice(1), 1);
  }

  function resetTextPreview() {
    $('code').textContent = '';
    preview.pendingLine = '';
    preview.renderedLines = 0;
    preview.highlightState = window.tshellHighlight.newState();
  }

  function appendRenderedLines(lines, language) {
    const fragment = document.createDocumentFragment();
    for (const line of lines) {
      const element = document.createElement('div');
      element.className = 'code-line';
      const number = document.createElement('div');
      number.className = 'line-number';
      preview.renderedLines += 1;
      number.textContent = String(preview.renderedLines);
      const code = document.createElement('div');
      code.className = 'line-code';
      code.innerHTML = highlightLine(line, language || 'text', preview.highlightState) || ' ';
      element.append(number, code);
      fragment.append(element);
    }
    $('code').append(fragment);
  }

  function appendTextChunk(content, language, done) {
    const normalized = String(content || '').replace(/\r\n/g, '\n').replace(/\r/g, '\n');
    const merged = preview.pendingLine + normalized;
    const lines = merged.split('\n');
    preview.pendingLine = done ? '' : (lines.pop() || '');
    if (!done && normalized.endsWith('\n') && lines[lines.length - 1] === '') lines.pop();
    appendRenderedLines(lines, language);
  }

  function resetPreviewScroll() {
    const code = $('code');
    code.scrollTop = 0;
    code.scrollLeft = 0;
    requestAnimationFrame(() => { code.scrollTop = 0; code.scrollLeft = 0; });
  }

  function renderPreview(message) {
    // A range left over from a stray select-all would otherwise swallow the
    // lines rendered below and show the whole file highlighted.
    clearSelection();
    $('previewTitle').textContent = message.path || message.name || '';
    preview.side = message.side || preview.side;
    preview.path = message.path || '';
    preview.encoding = message.encoding || preview.encoding;
    preview.language = message.language || 'text';
    preview.dbfNextRecord = 0;
    preview.loading = false;
    preview.tableMode = false;
    $('previewEncoding').value = preview.encoding;

    if (message.unsupported) {
      preview.content = '';
      preview.done = true;
      preview.nextOffset = 0;
      $('code').innerHTML = '<div class="unsupported-preview">' + escapeHtml(message.message || S.unsupportedBinaryPreview) + '</div>';
      $('toggleCsvView').style.display = 'none';
    } else {
      preview.content = String(message.content || '');
      preview.done = Boolean(message.done);
      preview.nextOffset = Number(message.nextOffset) || 0;
      $('toggleCsvView').style.display = preview.language === 'csv' ? '' : 'none';
      $('toggleCsvView').textContent = S.tablePreview;
      resetTextPreview();
      appendTextChunk(preview.content, preview.language, preview.done);
    }
    $('preview').classList.add('open');
    resetPreviewScroll();
  }

  function renderDbfPreview(message) {
    clearSelection();
    $('previewTitle').textContent = message.path || message.name || '';
    preview.side = message.side || preview.side;
    preview.path = message.path || '';
    preview.encoding = message.encoding || preview.encoding;
    preview.language = 'dbf';
    preview.done = Boolean(message.done);
    preview.dbfNextRecord = Number(message.nextRecord) || 0;
    preview.loading = false;
    preview.tableMode = true;
    $('previewEncoding').value = preview.encoding;
    $('toggleCsvView').style.display = 'none';
    $('code').innerHTML = renderTable((message.fields || []).map((field) => field.name || ''), message.rows || [], 1);
    $('preview').classList.add('open');
    resetPreviewScroll();
  }

  function appendPreviewChunk(message) {
    if (message.path !== preview.path || message.encoding !== preview.encoding) return;
    preview.loading = false;
    preview.done = Boolean(message.done);
    preview.nextOffset = Number(message.nextOffset) || preview.nextOffset;
    preview.content += String(message.content || '');
    if (preview.tableMode) $('code').innerHTML = renderCsvTable(preview.content);
    else appendTextChunk(message.content || '', preview.language, preview.done);
  }

  function appendDbfChunk(message) {
    if (message.path !== preview.path || message.encoding !== preview.encoding || preview.language !== 'dbf') return;
    preview.loading = false;
    preview.done = Boolean(message.done);
    const startIndex = preview.dbfNextRecord + 1;
    preview.dbfNextRecord = Number(message.nextRecord) || preview.dbfNextRecord;
    const body = $('code').querySelector('tbody');
    if (!body) return;
    const fragment = document.createDocumentFragment();
    (message.rows || []).forEach((row, rowIndex) => {
      const tr = document.createElement('tr');
      const indexCell = document.createElement('td');
      indexCell.className = 'row-index';
      indexCell.textContent = String(startIndex + rowIndex);
      tr.append(indexCell);
      for (const cell of row) {
        const td = document.createElement('td');
        td.textContent = cell || '';
        tr.append(td);
      }
      fragment.append(tr);
    });
    body.append(fragment);
  }

  function loadMorePreviewIfNeeded() {
    const code = $('code');
    if (!preview.path || preview.done || preview.loading) return;
    if (code.scrollTop + code.clientHeight < code.scrollHeight - 700) return;
    preview.loading = true;
    if (preview.language === 'dbf') post('loadDbfChunk', { side: preview.side, path: preview.path, encoding: preview.encoding, recordOffset: preview.dbfNextRecord });
    else post('loadTextChunk', { side: preview.side, path: preview.path, encoding: preview.encoding, offset: preview.nextOffset });
  }

  function applyPreviewFontSize() {
    $('code').style.setProperty('--preview-font-size', state.fontSize + 'px');
    $('previewFontSize').textContent = state.fontSize + 'px';
  }

  $('code').addEventListener('scroll', loadMorePreviewIfNeeded);
  $('closePreview').onclick = () => $('preview').classList.remove('open');
  $('previewEncoding').onchange = () => {
    preview.encoding = $('previewEncoding').value;
    if (preview.path) openPreview(preview.side, preview.path);
  };
  $('toggleCsvView').onclick = () => {
    if (preview.language !== 'csv') return;
    preview.tableMode = !preview.tableMode;
    $('toggleCsvView').textContent = preview.tableMode ? S.textPreview : S.tablePreview;
    if (preview.tableMode) {
      $('code').innerHTML = renderCsvTable(preview.content);
    } else {
      resetTextPreview();
      appendTextChunk(preview.content, preview.language, preview.done);
    }
  };
  $('previewFontDown').onclick = () => { state.fontSize = Math.max(8, state.fontSize - 1); saveState(); applyPreviewFontSize(); };
  $('previewFontUp').onclick = () => { state.fontSize = Math.min(24, state.fontSize + 1); saveState(); applyPreviewFontSize(); };

  // -- log ----------------------------------------------------------------

  function logLineElement(text) {
    const line = document.createElement('div');
    line.className = 'log-line';
    line.dataset.raw = text;
    const match = /^\[([^\]]*)\]\s?([\s\S]*)$/.exec(text);
    if (match) {
      const time = document.createElement('span');
      time.className = 'log-time';
      time.textContent = match[1];
      const msg = document.createElement('span');
      msg.className = 'log-msg';
      msg.textContent = match[2];
      line.append(time, msg);
    } else {
      const msg = document.createElement('span');
      msg.className = 'log-msg';
      msg.textContent = text;
      line.append(msg);
    }
    return line;
  }

  function appendLog(text, formatted) {
    const log = $('log');
    const full = formatted ? text : '[' + new Date().toLocaleTimeString() + '] ' + text;
    const atBottom = log.scrollTop + log.clientHeight >= log.scrollHeight - 6;
    log.append(logLineElement(full));
    if (atBottom) log.scrollTop = log.scrollHeight;
  }

  $('clearLog').onclick = () => { $('log').textContent = ''; post('clearLog'); };

  // -- messages -----------------------------------------------------------

  function applyList(message) {
    const pane = panes[message.side];
    const samePath = pane.path === message.path;
    pane.path = message.path || '';
    pane.canGoUp = Boolean(message.canGoUp);
    pane.entries = message.entries || [];
    pane.el.path.value = pane.path;
    // A refresh keeps whatever is still there; entering a folder starts clean.
    const keep = samePath ? new Set(pane.entries.map((entry) => entry.path)) : null;
    pane.selected = keep
      ? new Set(Array.from(pane.selected).filter((path) => keep.has(path)))
      : new Set();
    sortRows(pane);
    if (!samePath) { pane.lead = 0; pane.anchor = 0; pane.el.list.scrollTop = 0; }
    else { pane.lead = clampIndex(pane, pane.lead); pane.anchor = clampIndex(pane, pane.anchor); }
    pane.dropIndex = -1;
    render(pane);
  }

  window.addEventListener('message', (event) => {
    const message = event.data;
    switch (message.type) {
      case 'list': applyList(message); break;
      case 'listFailed': panes[message.side].el.path.value = panes[message.side].path; break;
      case 'log': appendLog(message.text || '', Boolean(message.formatted)); break;
      case 'logSnapshot': {
        const log = $('log');
        log.textContent = '';
        for (const line of (message.lines || [])) log.append(logLineElement(line));
        log.scrollTop = log.scrollHeight;
        break;
      }
      case 'progress': showProgress(message); break;
      case 'progressEnd': endProgress(); break;
      case 'transferState':
        // Scanning a deep tree can take a moment before the first progress tick,
        // so the panel opens as soon as the job starts.
        if (message.active) showProgress({ phase: 'scanning', overall: {}, current: {} });
        else endProgress();
        break;
      case 'conflict': showConflict(message); break;
      case 'textPreview': renderPreview(message); break;
      case 'textChunk': appendPreviewChunk(message); break;
      case 'dbfPreview': renderDbfPreview(message); break;
      case 'dbfChunk': appendDbfChunk(message); break;
    }
  });

  window.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    if ($('preview').classList.contains('open')) {
      $('preview').classList.remove('open');
      event.stopPropagation();
    }
  }, true);

  /*
   * Text selection is only ever legitimate inside these regions. Everything
   * else -- rows, headers, toolbars -- is chrome, not content.
   */
  const SELECTABLE = '.log, .code, .dialog';

  function inSelectableRegion(node) {
    const element = node && node.nodeType === 1 ? node : node && node.parentElement;
    return Boolean(element && element.closest && element.closest(SELECTABLE));
  }

  /*
   * The host runs its own select-all inside the webview on Ctrl+A. A keydown
   * preventDefault cannot stop it, because it is not the key's default action --
   * that is what paints the log, the address bars and the preview blue.
   *
   * Judging such a selection by where it landed does not work: a stale range
   * swallows whatever is rendered into it next, which is how opening a preview
   * ended up fully highlighted. So the rule is about provenance instead of
   * place -- a selection survives only when the user dragged it out inside a
   * selectable region. Everything else is dropped the moment it appears.
   */
  let dragSelecting = false;
  let userMadeSelection = false;

  function clearSelection() {
    const selection = window.getSelection();
    if (selection && selection.rangeCount) selection.removeAllRanges();
    userMadeSelection = false;
  }

  /*
   * A selection inside an <input> is not part of the document selection in
   * Chromium -- the field owns it, and getSelection() cannot see or clear it.
   * That is why the address bars stayed highlighted after the log stopped.
   * A field that does not have focus has no business showing a selection, so
   * collapsing those is always safe, and it leaves a field the user is actually
   * typing in (or dragging across) alone.
   */
  /** Collapses the address bars' own selections, leaving `keepField` alone. */
  function collapseInputs(keepField) {
    [panes.local.el.path, panes.remote.el.path].forEach((input) => {
      if (!input || input === keepField) return;
      if (input.selectionStart !== input.selectionEnd) input.setSelectionRange(0, 0);
    });
  }

  function dropStraySelection() {
    collapseInputs(document.activeElement);
    if (dragSelecting || userMadeSelection) return;
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return;
    selection.removeAllRanges();
  }

  /*
   * Wipes every text selection except the one Ctrl+A actually asked for, then
   * re-asserts that one, since clearing the document range can take an address
   * bar's selection with it.
   *
   * `guardValue` stops this from fighting the user: the moment they type over
   * the selected path the value changes, and re-asserting stops.
   */
  function sweepSelection(keepField, guardValue) {
    const selection = window.getSelection();
    if (selection && selection.rangeCount) selection.removeAllRanges();
    collapseInputs(keepField);
    dragSelecting = false;
    userMadeSelection = false;
    if (keepField && document.activeElement === keepField && keepField.value === guardValue) {
      keepField.select();
    }
  }

  /*
   * After Ctrl+A the sweep runs every frame for a short while rather than at a
   * few fixed delays, so if the host's select-all does slip through it is
   * cleared on the very next frame instead of staying up long enough to see.
   */
  let sweepDeadline = 0;
  let sweepRunning = false;
  let sweepField = null;
  let sweepGuardValue = '';

  function sweepTick() {
    sweepSelection(sweepField, sweepGuardValue);
    if (performance.now() < sweepDeadline) { requestAnimationFrame(sweepTick); return; }
    sweepRunning = false;
  }

  function startSweeps(field, guardValue) {
    sweepField = field;
    sweepGuardValue = guardValue;
    sweepDeadline = performance.now() + 400;
    sweepSelection(field, guardValue);
    if (sweepRunning) return;
    sweepRunning = true;
    requestAnimationFrame(sweepTick);
  }

  document.addEventListener('mousedown', (event) => {
    // Any click ends the post-Ctrl+A sweeping; the user is driving again.
    sweepDeadline = 0;
    dragSelecting = inSelectableRegion(event.target);
    userMadeSelection = false;
  }, true);

  document.addEventListener('mouseup', () => {
    const selection = window.getSelection();
    userMadeSelection = dragSelecting && Boolean(selection) && !selection.isCollapsed;
    dragSelecting = false;
  }, true);

  document.addEventListener('selectionchange', dropStraySelection);

  /*
   * Ctrl+A follows focus: in a file list it selects every entry, in an address
   * bar it selects the whole path.
   *
   * Both are performed here rather than left to the browser. Handing the key to
   * a focused field was what still let the host paint the other address bar and
   * the column headers blue, because the host answers Ctrl+A with a
   * document-wide select-all of its own that no preventDefault can stop. Doing
   * the work explicitly means the intended selection is the only one that
   * survives, whenever the host's version happens to land.
   */
  window.addEventListener('keydown', (event) => {
    const isSelectAll = (event.ctrlKey || event.metaKey) && (event.key === 'a' || event.key === 'A' || event.code === 'KeyA');
    if (!isSelectAll || event.altKey) return;
    if ($('preview').classList.contains('open') || $('conflictModal').classList.contains('open')) return;

    event.preventDefault();
    // Capture phase on window is the earliest hook in the document, ahead of
    // the host's own keydown listener. Stopping the event here means the key is
    // never forwarded to the workbench, so its select-all command never runs and
    // there is nothing to flash before the sweep can clear it.
    event.stopImmediatePropagation();
    event.stopPropagation();

    const target = event.target;
    const field = target === panes.local.el.path || target === panes.remote.el.path ? target : null;

    if (field) {
      field.select();
    } else {
      const pane = panes[activeSide];
      selectAll(pane);
      paintSelection(pane);
    }
    startSweeps(field, field ? field.value : '');
  }, true);

  window.addEventListener('resize', () => { render(panes.local); render(panes.remote); });

  // -- boot ---------------------------------------------------------------

  applyStrings();
  applyPreviewFontSize();
  $('previewEncoding').value = preview.encoding;
  if (state.split) $('panes').style.setProperty('--split', state.split + 'px');
  if (state.logHeight) document.body.style.setProperty('--log-height', state.logHeight + 'px');
  wirePane(panes.local);
  wirePane(panes.remote);
  applyColumnWidths(panes.local);
  applyColumnWidths(panes.remote);
  updateSortHeader(panes.local);
  updateSortHeader(panes.remote);
  panes.local.el.pane.classList.add('active');
  post('ready');
}());
