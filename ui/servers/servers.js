(function () {
  'use strict';

  const vscode = acquireVsCodeApi();
  const boot = window.__tshellServers || {};
  /*
   * Not const: this view is the one the language setting is changed from, and it
   * is pushed a fresh table with every state update, so it re-labels itself
   * without waiting to be reopened.
   */
  let S = boot.strings || {};

  const $ = (id) => document.getElementById(id);
  const post = (type, payload) => vscode.postMessage(Object.assign({ type: type }, payload || {}));
  const text = (key) => S[key] || key;

  let state = { groups: [], passwords: {}, privateKeyPassphrases: {}, aiEnabled: true };
  let selectedServerId = '';
  /*
   * Which groups the user has folded away. Kept here rather than read off the
   * DOM because render() rebuilds the tree from nothing: the open attribute is
   * gone by the time the next render would want to know what it said. Collapsed
   * rather than expanded, so a group that has never been touched -- including one
   * that has just been added -- is open, which is the useful default.
   */
  const collapsedGroups = new Set();
  /*
   * What the filter box holds. Kept here for the same reason as the fold state:
   * render() rebuilds the tree from nothing and has to be able to ask what it
   * is filtering by, and a state push from the host re-renders without going
   * anywhere near the input.
   */
  let query = '';

  function applyStrings() {
    document.querySelectorAll('[data-i18n]').forEach((el) => { el.textContent = S[el.dataset.i18n] || ''; });
    document.querySelectorAll('[data-i18n-placeholder]').forEach((el) => { el.placeholder = S[el.dataset.i18nPlaceholder] || ''; });
    // An icon button says what it does through its tooltip and its label.
    document.querySelectorAll('[data-i18n-title]').forEach((el) => {
      const label = S[el.dataset.i18nTitle] || '';
      el.title = label;
      el.setAttribute('aria-label', label);
    });
  }

  function status(message) { $('status').textContent = message || ''; }

  // -- context menu ---------------------------------------------------------

  function hideContextMenu() { $('contextMenu').classList.remove('open'); }

  /*
   * Items are `[label, action, icon, danger]`. The icon is the name of a glyph
   * in the shared sprite and the last two are optional -- a menu of nothing but
   * words still works, it just reads slower than one where the shape of the
   * thing is on the row beside its name.
   */
  function showContextMenu(event, items) {
    const menu = $('contextMenu');
    menu.replaceChildren();
    for (const entry of items) {
      // A bare '-' is a rule between two groups of items.
      if (entry === '-') {
        const rule = document.createElement('div');
        rule.className = 'c-menu-sep';
        menu.append(rule);
        continue;
      }
      const [label, action, icon, danger] = entry;
      const item = document.createElement('div');
      item.className = 'context-item c-menu-item' + (danger ? ' c-danger' : '');
      if (icon) item.append(window.tshellIcon(icon, 'c-icon'));
      const words = document.createElement('span');
      words.textContent = label;
      item.append(words);
      item.onclick = () => { hideContextMenu(); action(); };
      menu.append(item);
    }
    /*
     * Shown first, then measured, then moved: `display: none` measures as
     * nothing, and a guess at the size does not survive a menu whose length
     * depends on what was clicked. The row menu is seven items and two rules
     * where the guess allowed for 160px of it, so its last entry -- Delete --
     * was cut off by the bottom of the panel. This is an iframe: what hangs
     * past its edge is clipped, not drawn over the window.
     */
    /*
     * Back to the corner before measuring. The menu is one element reused for
     * every click, and a fixed box sized to its contents can only be as wide as
     * the room left of it -- measured where the last click put it, a menu near
     * the right edge measures narrower than it is about to be drawn.
     */
    menu.style.left = '0px';
    menu.style.top = '0px';
    menu.classList.add('open');
    const rect = menu.getBoundingClientRect();
    const fit = (want, size, room) => Math.max(0, Math.min(want, room - size - 4));
    menu.style.left = fit(event.clientX, rect.width, window.innerWidth) + 'px';
    menu.style.top = fit(event.clientY, rect.height, window.innerHeight) + 'px';
  }

  // -- tree -----------------------------------------------------------------

  /*
   * Moving the selection moves a class and nothing else. It used to rebuild the
   * tree, which is how selecting a server came to unfold every group, and which
   * also threw away the scroll position on the way.
   */
  function select(serverId) {
    selectedServerId = serverId;
    document.querySelectorAll('.server').forEach((row) => {
      row.classList.toggle('selected', row.dataset.serverId === serverId);
    });
  }

  function groupNode(group, count) {
    const wrap = document.createElement('details');
    wrap.className = 'group';
    // Read back by the drop code, which starts from an event and has to work out
    // which group it happened over without a closure to ask.
    wrap.dataset.groupId = group.id;
    /*
     * A filter that left groups folded would be a filter that hides its own
     * results. While one is running every group on screen is open and the fold
     * state is left untouched -- so clearing the box puts the tree back exactly
     * as the user had arranged it, rather than unfolded everywhere.
     */
    wrap.open = query ? true : !collapsedGroups.has(group.id);
    wrap.ontoggle = () => {
      if (query) return;
      if (wrap.open) collapsedGroups.delete(group.id); else collapsedGroups.add(group.id);
    };

    const head = document.createElement('summary');
    head.className = 'group-head';
    const chevron = window.tshellIcon('chevron', 'chevron');
    const name = document.createElement('div');
    name.className = 'group-name';
    // Already resolved by the host, including the default group's own label --
    // deciding it here, off the group's id, is what used to make that one group
    // impossible to rename.
    name.textContent = group.name;
    const badge = document.createElement('span');
    badge.className = 'group-count';
    badge.textContent = String(count);
    head.oncontextmenu = (event) => {
      event.preventDefault();
      event.stopPropagation();
      showContextMenu(event, [
        [text('addServer'), () => openServerDialog(group.id), 'plus'],
        [text('renameGroup'), () => post('requestRenameGroup', { groupId: group.id }), 'edit'],
        [text('addGroup'), () => post('requestAddGroup'), 'new-folder'],
        [text('deleteGroup'), () => post('requestDeleteGroup', { groupId: group.id }), 'trash', true]
      ]);
    };
    head.append(chevron, name, badge);
    // The handle for reordering groups is the group's own header.
    head.draggable = !query;
    head.ondragstart = (event) => beginDrag(event, { kind: 'group', groupId: group.id }, wrap);
    head.ondragend = endDrag;
    wrap.append(head);
    return wrap;
  }

  function serverRow(group, server) {
    const row = document.createElement('div');
    row.className = 'server';
    row.tabIndex = 0;
    row.dataset.serverId = server.id;
    if (selectedServerId === server.id) row.classList.add('selected');

    const icon = document.createElement('span');
    icon.className = 'server-icon';
    icon.title = server.name || server.host;

    const main = document.createElement('div');
    main.className = 'server-main';
    main.title = text('doubleClickConnect');
    main.onclick = () => select(server.id);
    main.ondblclick = () => post('openTerminal', { groupId: group.id, serverId: server.id });

    const title = document.createElement('div');
    title.className = 'server-name';
    title.textContent = server.name || server.host;
    const meta = document.createElement('div');
    meta.className = 'server-meta';
    meta.textContent = server.username + '@' + server.host + ':' + server.port;
    main.append(title, meta);

    row.oncontextmenu = (event) => {
      event.preventDefault();
      event.stopPropagation();
      select(server.id);
      /*
       * What to open, then what to change, then what to destroy -- in that
       * order, with a rule between the three. Opening is what a person came to
       * this menu for nine times in ten, and it was reachable only by knowing
       * that a double-click starts a terminal; the other two ways in were on
       * the title bar and needed a terminal already in front.
       */
      showContextMenu(event, [
        [text('openTerminal'), () => post('openTerminal', { groupId: group.id, serverId: server.id }), 'terminal'],
        [text('fileTransfer'), () => post('openTransfer', { groupId: group.id, serverId: server.id }), 'transfer'],
        ...(state.aiEnabled ? [[text('agentTitle'), () => post('openAssistant', { groupId: group.id, serverId: server.id }), 'ai']] : []),
        '-',
        [text('edit'), () => openServerDialog(group.id, server), 'edit'],
        // What the assistant remembers is only on offer while there is one.
        ...(state.aiEnabled ? [[text('memoryTitle'), () => post('openMemory', { serverId: server.id }), 'memory']] : []),
        '-',
        [text('delete'), () => post('requestDeleteServer', { groupId: group.id, serverId: server.id }), 'trash', true]
      ]);
    };

    row.draggable = !query;
    row.ondragstart = (event) =>
      beginDrag(event, { kind: 'server', groupId: group.id, serverId: server.id }, row);
    row.ondragend = endDrag;

    row.append(icon, main);
    return row;
  }

  // -- drag and drop --------------------------------------------------------

  /*
   * Reordering by hand: groups among groups, servers within a group, and servers
   * from one group to another.
   *
   * The order in the file *is* the order on screen -- both are the order of an
   * array -- so a drop is one index, and the host does the rest. What is decided
   * here is only which index the pointer is asking for.
   *
   * Two things are deliberately not draggable. One is anything at all while the
   * filter box has text in it: the rows on screen are then a subset, two
   * adjacent ones can have five hidden between them, and "put it here" has no
   * answer that would still be true once the filter is cleared. The other is a
   * drop onto the row being dragged, which would be a move to where it already
   * is; refusing to draw a line there says so before the mouse comes up.
   */

  let dragging = null;
  let springGroupId = '';
  let springTimer = 0;
  // Held direction and the interval running it. dragover keeps firing while the
  // pointer is still, but in bursts -- scrolling off it would go in lurches.
  let scrollDy = 0;
  let scrollTimer = 0;

  function beginDrag(event, what, element) {
    dragging = what;
    event.dataTransfer.effectAllowed = 'move';
    // Nothing reads it back -- dragover cannot -- but a drag with no data on it
    // does not start in every engine.
    event.dataTransfer.setData('text/plain', what.serverId || what.groupId);
    element.classList.add('dragging');
  }

  function clearMarks() {
    document.querySelectorAll('.drop-before, .drop-after, .drop-into').forEach((el) => {
      el.classList.remove('drop-before', 'drop-after', 'drop-into');
    });
  }

  function cancelSpring() {
    clearTimeout(springTimer);
    springTimer = 0;
    springGroupId = '';
  }

  function autoScroll(dy) {
    if (dy === scrollDy) return;
    clearInterval(scrollTimer);
    scrollTimer = 0;
    scrollDy = dy;
    if (dy) {
      scrollTimer = setInterval(() => { document.scrollingElement.scrollTop += dy; }, 16);
    }
  }

  function endDrag() {
    dragging = null;
    cancelSpring();
    autoScroll(0);
    clearMarks();
    document.querySelectorAll('.dragging').forEach((el) => el.classList.remove('dragging'));
  }

  function indexOfGroup(groupId) {
    return state.groups.findIndex((group) => group.id === groupId);
  }

  /** Below the middle of `el` is a drop after it, above it is a drop before. */
  function isAfter(event, el) {
    const box = el.getBoundingClientRect();
    return event.clientY > box.top + box.height / 2;
  }

  /*
   * What the pointer is currently asking for: the element to draw the line on,
   * which edge of it, and the index to send. Null means nothing valid is under
   * the pointer, and nothing valid is what "no line, no drop" is drawn from.
   *
   * Indices count the gaps in the list as it is drawn -- the list the dragged
   * row is still part of. The host owns the correction for the gap it leaves.
   */
  function dropPlan(event) {
    if (!dragging) return null;
    const groupEl = event.target.closest('.group');
    if (!groupEl) return null;
    const groupId = groupEl.dataset.groupId;

    if (dragging.kind === 'group') {
      if (groupId === dragging.groupId) return null;
      const at = indexOfGroup(groupId);
      if (at < 0) return null;
      /*
       * Measured against the whole group rather than its header: an unfolded
       * group with eight servers in it is tall, and a line that flipped from
       * "before" to "after" at the header's midpoint would be answering about a
       * strip of it the pointer left long ago.
       */
      const after = isAfter(event, groupEl);
      return { kind: 'group', mark: groupEl, edge: after ? 'after' : 'before', toIndex: at + (after ? 1 : 0) };
    }

    const group = state.groups[indexOfGroup(groupId)];
    if (!group) return null;

    const row = event.target.closest('.server');
    if (row) {
      if (row.dataset.serverId === dragging.serverId) return null;
      const at = group.servers.findIndex((server) => server.id === row.dataset.serverId);
      if (at < 0) return null;
      const after = isAfter(event, row);
      return {
        kind: 'server',
        mark: row,
        edge: after ? 'after' : 'before',
        toGroupId: groupId,
        toIndex: at + (after ? 1 : 0)
      };
    }

    // The header, or the "no servers" line of an empty group: the top of it.
    // For a folded group this is the only place a drop can land, which is what
    // makes the line under the header the right thing to draw.
    const empty = event.target.closest('.empty');
    return {
      kind: 'server',
      mark: empty || groupEl.querySelector('.group-head'),
      edge: empty ? 'before' : 'into',
      toGroupId: groupId,
      toIndex: 0,
      groupEl: groupEl
    };
  }

  /*
   * Hovering a folded group opens it, so that a server can be put somewhere in
   * particular rather than only "in there". It stays open afterwards: it is now
   * where the dragged server lives, and folding it back would hide the result of
   * the drop. Letting the ordinary toggle handler record that is the point --
   * the next render has to agree, or the group would close under the server.
   */
  function spring(plan) {
    const el = plan.groupEl;
    if (!el || el.open) { cancelSpring(); return; }
    if (springGroupId === plan.toGroupId) return;
    cancelSpring();
    springGroupId = plan.toGroupId;
    springTimer = setTimeout(() => { el.open = true; cancelSpring(); }, 600);
  }

  /** Near the top or bottom of the window, keep scrolling that way. */
  function edgeSpeed(event) {
    if (event.clientY < 28) return -10;
    if (window.innerHeight - event.clientY < 28) return 10;
    return 0;
  }

  /*
   * Name, host and username all count as a match, because which of the three a
   * person remembers a machine by depends on the machine: the box you named
   * `prod-web-1`, the one you only ever think of as .11, and the one that is
   * "the one I get into as deploy".
   */
  function matches(server, needle) {
    return (server.name || '').toLowerCase().includes(needle)
      || (server.host || '').toLowerCase().includes(needle)
      || (server.username || '').toLowerCase().includes(needle);
  }

  function render() {
    const root = $('groups');
    root.replaceChildren();

    const needle = query.trim().toLowerCase();
    let matched = 0;

    for (const group of state.groups) {
      /*
       * A group whose own name matches keeps all its servers. Typing the name
       * of a group is how you ask for that group, and filtering its contents
       * down to the ones that happen to repeat the name in their own would
       * answer a question nobody asked.
       */
      const groupHit = !!needle && (group.name || '').toLowerCase().includes(needle);
      const servers = !needle || groupHit
        ? group.servers
        : group.servers.filter((server) => matches(server, needle));

      if (needle && !servers.length) continue;
      matched += servers.length;

      const wrap = groupNode(group, servers.length);
      const children = document.createElement('div');
      children.className = 'children';
      if (!servers.length) {
        const empty = document.createElement('div');
        empty.className = 'empty';
        empty.textContent = text('noServers');
        children.append(empty);
      }
      for (const server of servers) children.append(serverRow(group, server));
      wrap.append(children);
      root.append(wrap);
    }

    // An empty result is a result. Without this the panel just goes blank and
    // reads as a panel that has broken rather than one that found nothing.
    if (needle && !matched) {
      const empty = document.createElement('div');
      empty.className = 'empty';
      empty.textContent = text('noMatches');
      root.append(empty);
    }
  }

  // -- server dialog --------------------------------------------------------

  function openServerDialog(groupId, server) {
    $('serverDialogTitle').textContent = server ? text('editServer') : text('newServer');
    $('serverGroupId').value = groupId;
    $('serverId').value = (server && server.id) || '';
    $('serverName').value = (server && server.name) || '';
    $('host').value = (server && server.host) || '';
    $('port').value = (server && server.port) || 22;
    $('username').value = (server && server.username) || '';
    $('authType').value = (server && server.authType) || (server && server.privateKeyPath ? 'privateKey' : 'password');
    $('password').value = server ? (state.passwords[server.id] || '') : '';
    $('privateKeyPath').value = (server && server.privateKeyPath) || '';
    $('privateKeyPassphrase').value = server ? (state.privateKeyPassphrases[server.id] || '') : '';
    $('encoding').value = (server && server.encoding) || 'utf-8';
    updateAuthFields();
    $('serverModal').classList.add('open');
  }

  /** Only the fields the chosen authentication actually uses are on screen. */
  function updateAuthFields() {
    const privateKey = $('authType').value === 'privateKey';
    const show = (id, visible) => { $(id).style.display = visible ? '' : 'none'; };
    show('passwordLabel', !privateKey);
    show('password', !privateKey);
    show('privateKeyPathLabel', privateKey);
    show('privateKeyPath', privateKey);
    show('privateKeyPassphraseLabel', privateKey);
    show('privateKeyPassphrase', privateKey);
  }

  function closeServerDialog() { $('serverModal').classList.remove('open'); }

  function saveServer() {
    const server = {
      id: $('serverId').value,
      name: $('serverName').value.trim(),
      host: $('host').value.trim(),
      port: Number($('port').value) || 22,
      username: $('username').value.trim(),
      authType: $('authType').value,
      privateKeyPath: $('privateKeyPath').value.trim(),
      encoding: $('encoding').value
    };
    if (!server.host || !server.username) { status(text('hostUserRequired')); return; }
    if (server.authType === 'privateKey' && !server.privateKeyPath) { status(text('privateKeyRequired')); return; }
    post(server.id ? 'updateServer' : 'addServer', {
      groupId: $('serverGroupId').value,
      server: server,
      password: $('password').value,
      privateKeyPassphrase: $('privateKeyPassphrase').value
    });
    closeServerDialog();
  }

  // -- wiring ---------------------------------------------------------------

  (function filterBox() {
    const box = $('search');
    const clear = $('searchClear');

    function apply(value) {
      query = value;
      clear.hidden = !value;
      render();
    }

    box.oninput = () => apply(box.value);

    /*
     * Escape empties the box rather than only blurring it. The box is a filter,
     * so the state a user wants back is the whole list -- and leaving the text
     * behind while they look at the tree is how a panel ends up "missing"
     * servers that are only filtered out.
     */
    box.onkeydown = (event) => {
      if (event.key !== 'Escape') return;
      if (!box.value) return;
      event.stopPropagation();
      box.value = '';
      apply('');
    };

    clear.onclick = () => {
      box.value = '';
      apply('');
      box.focus();
    };
  }());

  /*
   * One listener on the tree rather than one per row: render() throws the rows
   * away and builds them again on every state push, and these have to outlive
   * that. The rows carry only what starts a drag.
   */
  (function dropTarget() {
    const root = $('groups');

    root.addEventListener('dragover', (event) => {
      if (!dragging) return;
      const plan = dropPlan(event);
      clearMarks();
      autoScroll(edgeSpeed(event));
      if (!plan) { cancelSpring(); return; }
      // Without this the drop never arrives: the default is to refuse.
      event.preventDefault();
      event.dataTransfer.dropEffect = 'move';
      plan.mark.classList.add('drop-' + plan.edge);
      spring(plan);
    });

    root.addEventListener('drop', (event) => {
      if (!dragging) return;
      event.preventDefault();
      // Read before endDrag, which is what clears them both.
      const plan = dropPlan(event);
      const from = dragging;
      endDrag();
      if (!plan) return;
      if (plan.kind === 'group') {
        post('moveGroup', { groupId: from.groupId, toIndex: plan.toIndex });
      } else {
        /*
         * Unfold where it landed. Dropping straight onto a folded group without
         * waiting for it to spring open is a legal move, and the render that
         * follows would otherwise fold the server out of sight the moment it
         * arrived -- the one report the user gets that the drop worked.
         */
        collapsedGroups.delete(plan.toGroupId);
        post('moveServer', {
          fromGroupId: from.groupId,
          serverId: from.serverId,
          toGroupId: plan.toGroupId,
          toIndex: plan.toIndex
        });
      }
    });
  }());

  /*
   * The row's own dragend covers a drop and a drag let go over the tree. This
   * covers the rest: Escape, and a release outside the panel -- both of which
   * would otherwise leave the row dimmed and a line drawn under nothing.
   */
  document.addEventListener('dragend', endDrag);

  $('cancelServer').onclick = closeServerDialog;
  $('saveServer').onclick = saveServer;
  $('authType').onchange = updateAuthFields;
  $('serverModal').onclick = (event) => { if (event.target === $('serverModal')) closeServerDialog(); };

  document.body.addEventListener('click', hideContextMenu);
  window.addEventListener('blur', hideContextMenu);
  window.addEventListener('keydown', (event) => { if (event.key === 'Escape') hideContextMenu(); });

  // Blank space has its own menu: the two things that are not about one row.
  document.body.addEventListener('contextmenu', (event) => {
    if (event.target.closest('.group-head') || event.target.closest('.server') || event.target.closest('.modal')) return;
    // The filter box keeps its own menu -- cut, copy, paste. A right-click in a
    // text field asking "add a group?" is the panel talking over the field.
    if (event.target.closest('.search')) return;
    event.preventDefault();
    showContextMenu(event, [
      [text('addGroup'), () => post('requestAddGroup')],
      [text('openConfig'), () => post('openConfig')]
    ]);
  });

  window.addEventListener('message', (event) => {
    const message = event.data;
    if (message.type === 'state') {
      if (message.strings) S = message.strings;
      state = {
        groups: message.groups || [],
        passwords: message.passwords || {},
        privateKeyPassphrases: message.privateKeyPassphrases || {},
        aiEnabled: message.aiEnabled !== false
      };
      status('');
      applyStrings();
      render();
      return;
    }
    if (message.type === 'error') status(message.message);
  });

  applyStrings();
}());
