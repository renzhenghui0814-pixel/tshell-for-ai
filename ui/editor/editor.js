/*
 * The editor pane.
 *
 * One file, held whole, drawn as three layers that agree on every character:
 * a textarea that owns the text and shows only its caret and selection, a
 * painted layer under it that shows the text in colour, and a gutter beside
 * both. See editor.css for why they cannot be one thing.
 *
 * Nothing here touches Tauri. Every read and every write is a message to the
 * shell, which owns the only IPC channel in the window.
 *
 * WHAT THIS IS NOT: a code editor. There is no autocomplete, no bracket
 * matching, no multiple cursors and no undo beyond the browser's own. It exists
 * because tshell asks the user to edit half a dozen files it owns and had been
 * answering that with "open it in something else", and because a log you have to
 * leave the window to read is a log you do not read.
 */
(function () {
  'use strict';

  var vscode = acquireVsCodeApi();
  var boot = window.tshellBootstrap || {};
  var S = boot.strings || {};
  var H = window.tshellHighlight;

  var $ = function (id) { return document.getElementById(id); };
  var post = function (type, payload) {
    vscode.postMessage(Object.assign({ type: type }, payload || {}));
  };
  var t = function (key, arg) {
    var value = S[key] || key;
    return arg === undefined ? value : String(value).replace('{0}', String(arg));
  };

  var scroll = $('scroll');
  var gutter = $('gutter');
  var paint = $('paint');
  var input = $('input');
  var findbar = $('findbar');
  var findInput = $('findInput');
  var saveButton = $('save');

  /*
   * The height of one line, read from the stylesheet rather than repeated here.
   *
   * Find scrolls to a match by multiplying it, so a number that disagreed with
   * the CSS would put every match a growing distance off screen -- the kind of
   * wrong that looks like a scrolling bug rather than a stale constant.
   */
  var LINE_H = 20;
  var PAD_TOP = 10;

  var doc = {
    side: boot.side || 'local',
    path: boot.path || '',
    encoding: boot.encoding || 'utf8',
    language: 'text',
    stamp: { size: 0, modified: 0 },
    /* '', 'binary', 'tooBig' or 'forced'. Anything but '' means read-only. */
    readOnly: boot.readOnly ? 'forced' : '',
    lines: [],
    painted: [],
    saved: '',
    /* Read-only streaming, for a file too large to hold for editing. */
    streaming: false,
    done: true,
    nextOffset: 0,
    total: 0,
    /*
     * 'text' or 'table'. A DBF has only the second -- it is records read by
     * field, and there is no text underneath to fall back to. A CSV has both,
     * and opens as a table because that is what someone opening a CSV is
     * looking at; its text is a mode away and editable there.
     */
    mode: 'text',
    table: { fields: [], rows: [], next: 0, done: true, loading: false }
  };

  var lineEls = [];
  var numberEls = [];

  /** The last dirty state the shell was told, so it is only told the changes. */
  var told = false;

  // -- text ------------------------------------------------------------------

  /*
   * The size of the code, shared by every editor pane through localStorage.
   *
   * Not the per-pane store host.js offers: someone who makes the text bigger is
   * saying how they want to read files, not how they want to read this one. It
   * outlives the tab for the same reason.
   */
  var FONT_MIN = 8;
  var FONT_MAX = 24;
  var FONT_KEY = 'tshell:editorFontSize';
  var fontSize = 12.5;

  function loadFont() {
    try {
      var saved = parseFloat(window.localStorage.getItem(FONT_KEY));
      if (saved >= FONT_MIN && saved <= FONT_MAX) fontSize = saved;
    } catch (error) {
      // A store that will not answer is not worth failing a page load over.
    }
  }

  function applyFont() {
    document.documentElement.style.setProperty('--code-size', fontSize + 'px');
    $('fontValue').textContent = fontSize + 'px';
    try {
      window.localStorage.setItem(FONT_KEY, String(fontSize));
    } catch (error) {
      // As above.
    }
    // The line height is a ratio of the size, so it moved. Everything that
    // scrolls to a line multiplies by it.
    measure();
  }

  function measure() {
    var style = window.getComputedStyle(document.querySelector('.rows'));
    var height = parseFloat(style.lineHeight);
    if (height > 0) LINE_H = height;
    var padding = parseFloat(window.getComputedStyle(paint).paddingTop);
    if (padding >= 0) PAD_TOP = padding;
  }

  /*
   * Redraw the painted layer and the gutter from the textarea.
   *
   * Highlighting restarts at the first line that differs, not at the top: the
   * highlighter carries state between lines -- whether a block comment or a
   * docstring is still open -- so it cannot start in the middle without one, and
   * the state before an unchanged line is by definition unchanged.
   *
   * The DOM is then touched only where the HTML actually differs. Typing changes
   * one line and writes one element; inserting a line at the top of a large file
   * writes all of them, because every line below has moved and there is no key
   * to match them by. That case is rare and the cost is one pass.
   */
  function repaint() {
    var next = input.value.split('\n');

    var start = 0;
    while (start < next.length && start < doc.lines.length && next[start] === doc.lines[start]) {
      start += 1;
    }

    var state = H.newState();
    // Replay the lines before the first change to rebuild the carry state. They
    // are not re-rendered; only the highlighter walks them.
    for (var replay = 0; replay < start; replay += 1) {
      H.highlightLine(next[replay], doc.language, state);
    }

    for (var i = start; i < next.length; i += 1) {
      var html = H.highlightLine(next[i], doc.language, state) || ' ';
      if (doc.painted[i] !== html) {
        doc.painted[i] = html;
        if (lineEls[i]) lineEls[i].innerHTML = html;
      }
    }

    doc.painted.length = next.length;
    doc.lines = next;
    fitRows(next.length);
  }

  /** Add or drop line elements so there is exactly one per line. */
  function fitRows(count) {
    while (lineEls.length < count) {
      var line = document.createElement('div');
      line.className = 'ln';
      line.innerHTML = doc.painted[lineEls.length] || ' ';
      paint.appendChild(line);
      lineEls.push(line);

      var number = document.createElement('b');
      number.textContent = String(numberEls.length + 1);
      gutter.appendChild(number);
      numberEls.push(number);
    }
    while (lineEls.length > count) {
      paint.removeChild(lineEls.pop());
      gutter.removeChild(numberEls.pop());
    }
  }

  // -- the table -------------------------------------------------------------

  /** Which second form this file has, or '' for a file that is only text. */
  function tableKind() {
    if (doc.language === 'dbf') return 'dbf';
    if (doc.language === 'csv') return 'csv';
    return '';
  }

  function setMode(mode) {
    doc.mode = mode;
    var table = mode === 'table';
    $('grid').hidden = !table;
    scroll.hidden = table;
    // Find works on the text, and in a table there is none on screen. Left open
    // it would be a search box over a document that is not being shown.
    if (table && !findbar.hidden) closeFind();
    if (table) drawTable();
    settle();
  }

  function drawTable() {
    if (doc.language === 'dbf') {
      drawGrid(doc.table.fields, doc.table.rows, 1);
      return;
    }
    var rows = parseCsv(input.value);
    if (!rows.length) { $('grid').replaceChildren(); return; }
    drawGrid(rows[0], rows.slice(1), 1);
  }

  /*
   * A CSV, by the rules a CSV actually follows: quotes protect commas and
   * newlines, and a doubled quote inside a quoted field is one quote.
   *
   * Carried over from the file panel's preview rather than rewritten. It is the
   * same job and it was already right; the only thing that changed is where the
   * table is drawn.
   */
  function parseCsv(content) {
    var rows = [];
    var row = [];
    var cell = '';
    var quoted = false;
    for (var index = 0; index < content.length; index += 1) {
      var char = content[index];
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

  /*
   * Built as elements rather than assembled as HTML.
   *
   * A cell holds whatever was in the file, and a file is not to be trusted with
   * markup: `textContent` puts the characters on screen as characters, where a
   * string of HTML would put a `<script>` in a spreadsheet cell into the page.
   */
  function drawGrid(headers, rows, from) {
    var table = document.createElement('table');

    var head = document.createElement('thead');
    var headRow = document.createElement('tr');
    var corner = document.createElement('th');
    corner.className = 'at';
    corner.textContent = '#';
    headRow.appendChild(corner);
    headers.forEach(function (name) {
      var cell = document.createElement('th');
      cell.textContent = String(name === undefined || name === null ? '' : name);
      headRow.appendChild(cell);
    });
    head.appendChild(headRow);

    var body = document.createElement('tbody');
    appendRows(body, headers.length, rows, from);

    table.append(head, body);
    $('grid').replaceChildren(table);
  }

  function appendRows(body, width, rows, from) {
    var fragment = document.createDocumentFragment();
    rows.forEach(function (row, index) {
      var tr = document.createElement('tr');
      var at = document.createElement('td');
      at.className = 'at';
      at.textContent = String(from + index);
      tr.appendChild(at);
      for (var i = 0; i < Math.max(row.length, width); i += 1) {
        var cell = document.createElement('td');
        cell.textContent = row[i] === undefined || row[i] === null ? '' : String(row[i]);
        tr.appendChild(cell);
      }
      fragment.appendChild(tr);
    });
    body.appendChild(fragment);
  }

  /** Another page of records, for a DBF that is longer than one request. */
  function grewTable(chunk) {
    doc.table.loading = false;
    doc.table.done = !!chunk.done;
    var first = !doc.table.fields.length;
    if (first) doc.table.fields = (chunk.fields || []).map(function (f) { return f.name || ''; });

    var from = doc.table.rows.length + 1;
    doc.table.rows = doc.table.rows.concat(chunk.rows || []);
    doc.table.next = Number(chunk.nextRecord) || doc.table.next;

    if (first) {
      drawGrid(doc.table.fields, doc.table.rows, 1);
      return;
    }
    var body = $('grid').querySelector('tbody');
    if (body) appendRows(body, doc.table.fields.length, chunk.rows || [], from);
  }

  function moreRecords() {
    if (doc.language !== 'dbf' || doc.table.done || doc.table.loading) return;
    doc.table.loading = true;
    post('editorTable', { recordOffset: doc.table.next });
  }

  $('grid').addEventListener('scroll', function () {
    var grid = $('grid');
    if (grid.scrollTop + grid.clientHeight * 2 >= grid.scrollHeight) moreRecords();
  });

  // -- what the shell sends --------------------------------------------------

  function show(file) {
    doc.language = file.language || 'text';
    doc.stamp = file.stamp || { size: 0, modified: 0 };
    if (!doc.readOnly) doc.readOnly = file.readOnly || '';
    doc.total = doc.stamp.size || 0;

    /*
     * A DBF is answered before anything is said about its text.
     *
     * `text_open` calls it binary, which is true and useless: it is a table, and
     * the reason it looks like nothing to a text reader is that it is not one.
     * The records come from the same command the file panel used to read them
     * with, a page at a time.
     */
    if (doc.language === 'dbf') {
      /*
       * Read-only, but not "not a text file" -- which is what the text reader
       * says about it and is the least useful true thing to put on screen next
       * to a table of its records.
       */
      doc.readOnly = 'table';
      doc.table = { fields: [], rows: [], next: 0, done: false, loading: true };
      setMode('table');
      post('editorTable', { recordOffset: 0 });
      return;
    }

    if (file.readOnly === 'binary') {
      // Nothing to show and nothing that showing it would help with.
      input.value = '';
      doc.lines = [];
      doc.painted = [];
      fitRows(0);
      say(t('editorBinary'), true);
      settle();
      return;
    }

    if (file.readOnly === 'tooBig') {
      /*
       * Streamed rather than refused. The panel this pane took over from could
       * open a 500MB log, and taking that away to gain an editor would be a
       * trade nobody asked for. It arrives through the preview commands, which
       * have read files of any size for as long as the panel has existed.
       */
      input.value = '';
      doc.streaming = true;
      doc.done = false;
      doc.nextOffset = 0;
      settle();
      more();
      return;
    }

    input.value = file.content || '';
    doc.saved = input.value;
    repaint();
    // A CSV opens as a table, because that is what someone opening a CSV came
    // to look at. Its text is one button away, and editable there.
    setMode(doc.language === 'csv' ? 'table' : 'text');
  }

  /** Another slice of a file that is only being shown. */
  function grew(chunk) {
    doc.streaming = false;
    if (chunk.binary) {
      say(t('editorBinary'), true);
      doc.done = true;
      return;
    }
    // Appended through the value rather than rebuilt, so the browser keeps the
    // selection a find may have put there.
    input.value += chunk.content || '';
    doc.nextOffset = chunk.nextOffset || 0;
    doc.done = !!chunk.done;
    repaint();
    if (doc.mode === 'table') drawTable();
    settle();
    if (findInput.value) find(0);

    /*
     * Keep pulling until there is something to scroll.
     *
     * The scroll handler is what asks for the next slice, and it cannot fire
     * while the content is shorter than the pane. Without this a large file
     * stops after its first chunk and looks like a file that is exactly 512KB
     * long -- which is the most convincing possible way to be wrong.
     */
    if (!doc.done && scroll.scrollHeight <= scroll.clientHeight * 2) more();
  }

  function more() {
    if (doc.done || doc.streaming) return;
    doc.streaming = true;
    post('editorMore', { offset: doc.nextOffset });
  }

  /*
   * Ask for the next slice once the reader is within a screen of the end.
   *
   * A screen rather than the very bottom: a request takes a round trip and, on
   * the remote side, a packet exchange with the server. Arriving at the last
   * line and then waiting is what the margin exists to avoid.
   */
  scroll.addEventListener('scroll', function () {
    if (doc.done) return;
    if (scroll.scrollTop + scroll.clientHeight * 2 >= scroll.scrollHeight) more();
  });

  // -- state on screen -------------------------------------------------------

  function dirty() {
    return !doc.readOnly && input.value !== doc.saved;
  }

  /** Everything the strip says about the file, worked out in one place. */
  function settle() {
    $('where').textContent = doc.path;
    $('where').title = doc.path;

    var badge = $('badge');
    var reason = doc.readOnly === 'tooBig' ? t('editorTooBig')
      : doc.readOnly === 'binary' ? t('editorBinary')
      : doc.readOnly ? t('editorReadOnly')
      : '';
    badge.textContent = reason;
    badge.hidden = !reason;

    var kind = tableKind();
    var button = $('mode');
    // Only a CSV has two forms to move between. A DBF has one, and a button
    // that switches to a view that cannot exist is a button that lies.
    button.hidden = kind !== 'csv';
    button.textContent = doc.mode === 'table' ? t('editorAsText') : t('editorAsTable');

    // Encoding still applies in a table -- it is how the bytes are read, and a
    // DBF holds text in its fields like anything else.
    $('encoding').value = doc.encoding === 'gb2312' ? 'gb2312' : 'utf8';

    input.readOnly = !!doc.readOnly;
    // Saving belongs to the text. A table is a reading of the buffer, not the
    // buffer, and there is nothing in it that an edit could have come from.
    saveButton.hidden = !!doc.readOnly || doc.mode === 'table';

    var now = dirty();
    saveButton.classList.toggle('dirty', now);
    // Only on the edge. `settle` runs on every keystroke, and the shell needs to
    // hear about the first one and the one that undoes it -- not the 4000 in
    // between, each of which would be a message carrying what it already knows.
    if (now !== told) {
      told = now;
      post('editorDirty', { dirty: now });
    }
  }

  /*
   * The strip at the bottom, which is for things that need answering.
   *
   * It used to carry progress as well -- opening, saving, saved -- and a bar
   * that is nearly always showing something routine is a bar nobody reads by
   * the time it says something that matters. Progress has somewhere better to
   * be: the dot on the tab appears when there is unsaved work and goes when
   * there is not, which is the same fact where it is already being looked for.
   */
  function say(text, bad) {
    var notice = $('notice');
    notice.textContent = text || '';
    notice.classList.toggle('bad', !!bad);
    notice.hidden = !text;
  }

  // -- find ------------------------------------------------------------------

  var hits = [];
  var at = -1;

  /*
   * Matches are shown by selecting them in the textarea, so what marks a match
   * is the browser's own selection.
   *
   * The alternative is cutting marks into the painted layer, which is HTML by
   * the time it gets there -- a plain replace over it would match inside a tag
   * name as readily as inside the text, and the first thing it would break is
   * searching for a word that also appears in a class name.
   */
  /** Every place `needle` occurs, case-insensitively. */
  function occurrences(needle) {
    var places = [];
    if (!needle) return places;
    var hay = input.value.toLowerCase();
    var want = needle.toLowerCase();
    var from = 0;
    for (;;) {
      var found = hay.indexOf(want, from);
      if (found < 0) break;
      places.push(found);
      // Overlapping matches count once each, from one character on, so
      // searching `aa` in `aaa` finds two rather than one.
      from = found + 1;
    }
    return places;
  }

  function scan() {
    var needle = findInput.value;
    hits = occurrences(needle);
    at = -1;
    findInput.classList.toggle('empty', !!needle && !hits.length);
    if (!hits.length) clearMarks();
    tally();
  }

  function tally() {
    var label = !findInput.value ? ''
      : !hits.length ? t('editorNoMatch')
      : (at + 1) + ' / ' + hits.length;
    $('tally').textContent = label;
  }

  function find(step) {
    if (!hits.length) { tally(); return; }
    if (at < 0) {
      // The first jump goes forward from where the caret is, so opening find in
      // the middle of a file does not send the reader back to the top.
      var from = input.selectionStart;
      at = 0;
      for (var i = 0; i < hits.length; i += 1) {
        if (hits[i] >= from) { at = i; break; }
      }
      if (step < 0) at = (at - 1 + hits.length) % hits.length;
    } else {
      at = (at + step + hits.length) % hits.length;
    }

    var start = hits[at];
    /*
     * The selection is set but focus is NOT taken.
     *
     * It used to be: `find` ran on every keystroke in the search box and focused
     * the textarea to make the browser draw the selection -- so typing a second
     * character was impossible without clicking back into the box first. Focus
     * belongs to whoever is typing, and while the search box is open that is the
     * search box.
     *
     * Which costs the selection its colour: an unfocused control draws one in
     * the platform's muted grey, if at all. So the line carries the mark instead
     * -- the whole line, because marking the characters exactly would mean
     * cutting into HTML that is already tags, and searching for a word that is
     * also a class name would light up the class name.
     */
    input.setSelectionRange(start, start + findInput.value.length);
    paintMarks();
    reveal(start);
    tally();
  }

  /* Which line elements currently hold marks, so they can be put back. */
  var marked = [];

  /*
   * How many matches are drawn at once, centred on the current one.
   *
   * Searching for `e` in a large file finds tens of thousands, and building a
   * mark for each on every keystroke is work nobody can see the result of --
   * only a few hundred lines are on screen. The window is centred on the
   * current match rather than taken from the top, so the one being looked at is
   * always among them.
   */
  var MARK_WINDOW = 400;

  /** Undo the marking by putting each line back to what the highlighter made. */
  function clearMarks() {
    marked.forEach(function (i) {
      if (lineEls[i]) lineEls[i].innerHTML = doc.painted[i];
    });
    marked = [];
  }

  /*
   * Draw the matches into the painted layer.
   *
   * On the DOM, not on the HTML string. By the time a line is on screen it is
   * elements and text nodes, and a text node holds the characters themselves --
   * `<` is one character there, not the four of `&lt;`. Searching the HTML
   * instead would match inside tag names and entity names, so looking for a word
   * that is also a token class would light up the markup rather than the text,
   * and the offsets would be wrong by however many entities came before.
   */
  function paintMarks() {
    show_(hits, findInput.value.length, at, 'hit');
  }

  /*
   * Draw a set of places into the painted layer.
   *
   * Two callers with the same need: the find bar, which has a current match and
   * navigates between them, and the selection echo, which has neither. One
   * painter, because two would be two places to get the offset arithmetic right
   * and only one of them would ever be exercised by a test.
   */
  function show_(places, length, current, kind) {
    clearMarks();
    if (!places.length || !length) return;

    var starts = lineStarts();
    var from = Math.max(0, (current < 0 ? 0 : current) - MARK_WINDOW);
    var to = Math.min(places.length, from + MARK_WINDOW * 2);

    /* Grouped by line, because marking is one pass over each line element. */
    var byLine = new Map();
    for (var h = from; h < to; h += 1) {
      var line = lineOf(starts, places[h]);
      if (!byLine.has(line)) byLine.set(line, []);
      byLine.get(line).push({
        start: places[h] - starts[line],
        end: places[h] - starts[line] + length,
        kind: h === current ? kind + ' now' : kind
      });
    }

    byLine.forEach(function (ranges, line) {
      if (!lineEls[line]) return;
      markIn(lineEls[line], ranges);
      marked.push(line);
    });
  }

  /** The character offset each line begins at. */
  function lineStarts() {
    var starts = new Array(doc.lines.length);
    var offset = 0;
    for (var i = 0; i < doc.lines.length; i += 1) {
      starts[i] = offset;
      offset += doc.lines[i].length + 1;
    }
    return starts;
  }

  /** Which line an offset falls on, by binary search over the line starts. */
  function lineOf(starts, index) {
    var low = 0;
    var high = starts.length - 1;
    while (low < high) {
      var mid = (low + high + 1) >> 1;
      if (starts[mid] <= index) low = mid;
      else high = mid - 1;
    }
    return low;
  }

  function lineAt(index) {
    return lineOf(lineStarts(), index);
  }

  /*
   * Wrap each range in a `<mark>`, working through the line's text nodes.
   *
   * Backwards -- last node first, and within a node the last range first --
   * because `splitText` leaves everything before the split alone. Going forwards
   * would invalidate every offset after the first mark.
   */
  function markIn(el, ranges) {
    var nodes = [];
    var walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    var offset = 0;
    var node;
    while ((node = walker.nextNode())) {
      nodes.push({ node: node, start: offset, end: offset + node.nodeValue.length });
      offset += node.nodeValue.length;
    }

    ranges.sort(function (left, right) { return left.start - right.start; });

    for (var n = nodes.length - 1; n >= 0; n -= 1) {
      var piece = nodes[n];
      for (var r = ranges.length - 1; r >= 0; r -= 1) {
        var start = Math.max(ranges[r].start, piece.start);
        var end = Math.min(ranges[r].end, piece.end);
        if (start >= end) continue;

        // Two splits leave three nodes in the tree: before, the match, after.
        // Only the middle one is wanted; the other two stay where they are.
        piece.node.splitText(end - piece.start);
        var middle = piece.node.splitText(start - piece.start);
        var mark = document.createElement('mark');
        mark.className = ranges[r].kind;
        middle.parentNode.replaceChild(mark, middle);
        mark.appendChild(middle);
      }
    }
  }

  /** Put the character at `index` in the middle of the pane. */
  function reveal(index) {
    var top = PAD_TOP + lineAt(index) * LINE_H;
    scroll.scrollTop = Math.max(0, top - scroll.clientHeight / 2);
  }

  /*
   * Light up every other place the selected text appears.
   *
   * The question "where else does this happen" is asked by selecting the thing
   * and looking, not by copying it into a search box -- which is why every
   * editor answers it without being asked. Nothing moves and nothing scrolls:
   * it marks what is already there and leaves the reader where they are.
   *
   * The find bar wins while it is open. Both draw into the same layer, and a
   * search whose results kept being replaced by whatever the caret last touched
   * would be a search that erases itself.
   */
  var echoed = '';

  function echo() {
    if (!findbar.hidden) return;
    var text = input.value.slice(input.selectionStart, input.selectionEnd);

    /*
     * A selection that spans lines is a passage, not a word, and marking every
     * other copy of a paragraph is not a question anyone asked. Whitespace is
     * excluded for the same reason from the other end: every line in the file
     * would light up.
     */
    var worth = text.length > 1 && text.indexOf('\n') < 0 && text.trim() !== '';
    if (!worth) {
      if (echoed) { echoed = ''; clearMarks(); }
      return;
    }
    if (text === echoed) return;
    echoed = text;
    show_(occurrences(text), text.length, -1, 'echo');
  }

  /*
   * `selectionchange` on the document rather than `select` on the textarea:
   * `select` does not fire when the caret is moved by an arrow key, so a
   * selection extended with Shift+Right would be marked only when it happened
   * to have been made with the mouse.
   */
  var echoSoon = null;
  document.addEventListener('selectionchange', function () {
    if (document.activeElement !== input) return;
    window.clearTimeout(echoSoon);
    // Dragging a selection across a file fires this per character. The marks are
    // worth drawing once the selection has stopped moving, not forty times on
    // the way there.
    echoSoon = window.setTimeout(echo, 90);
  });

  function openFind() {
    if (doc.mode === 'table') return;
    findbar.hidden = false;
    echoed = '';
    var selected = input.value.slice(input.selectionStart, input.selectionEnd);
    // A selection that spans lines is not a search term; it is a paragraph.
    if (selected && selected.indexOf('\n') < 0) findInput.value = selected;
    scan();
    if (hits.length) find(0);
    // Last, because `find` used to take the focus and this is the one place
    // where that was invisible: the box was focused, then emptied of focus, and
    // the first keystroke went into the file.
    findInput.focus();
    findInput.select();
  }

  function closeFind() {
    findbar.hidden = true;
    findInput.classList.remove('empty');
    hits = [];
    at = -1;
    clearMarks();
    // Now focus goes back, with the caret already on the last match found --
    // which is the reason someone was searching. That selection is a selection
    // like any other, so the echo picks it up from here.
    input.focus();
    echoed = '';
    echo();
  }

  // -- saving ----------------------------------------------------------------

  var saving = false;

  function save() {
    if (doc.readOnly || saving || !dirty()) return;
    saving = true;
    post('editorSave', { content: input.value, base: doc.stamp });
  }

  function saved(reply) {
    saving = false;
    if (reply.error) {
      say(t(reply.error) || reply.error, true);
      return;
    }
    if (reply.cancelled) {
      // The conflict question was answered "leave it". The buffer is untouched
      // and still unsaved, which is the state the user chose, and the mark on
      // the tab is already saying so.
      return;
    }
    doc.stamp = reply.stamp || doc.stamp;
    doc.saved = reply.content;
    settle();
  }

  // -- wiring ----------------------------------------------------------------

  input.addEventListener('input', function () {
    /*
     * Marks come off before the redraw, not after.
     *
     * `repaint` only rewrites the lines that changed, so simply forgetting the
     * marked lines would leave real `<mark>` elements in the DOM on every line
     * that did not -- and the next pass would wrap marks inside marks. Taking
     * them off first puts every line back to what the highlighter made of it,
     * which is exactly what `repaint` expects to be comparing against.
     */
    clearMarks();
    echoed = '';
    repaint();
    settle();
    if (!findbar.hidden) { scan(); paintMarks(); }
  });

  /*
   * Scroll is on the container, but a textarea keeps its own when it has focus
   * and the two would drift. The textarea never scrolls internally -- it is
   * exactly as large as its content -- so any scrollTop it reports is the
   * browser dragging the caret into view, and it belongs to the container.
   */
  input.addEventListener('scroll', function () {
    if (input.scrollTop) {
      scroll.scrollTop += input.scrollTop;
      input.scrollTop = 0;
    }
    if (input.scrollLeft) {
      scroll.scrollLeft += input.scrollLeft;
      input.scrollLeft = 0;
    }
  });

  input.addEventListener('keydown', function (event) {
    // Tab indents. Moving focus out of an editing surface is what Escape and
    // the mouse are for, and a file where Tab means something is a file where
    // Tab has to be typeable.
    if (event.key === 'Tab' && !event.ctrlKey && !event.altKey && !event.metaKey) {
      event.preventDefault();
      var start = input.selectionStart;
      var end = input.selectionEnd;
      // `execCommand` rather than rewriting `.value`, because it is the only
      // way to put text in that the browser records as an undoable step.
      document.execCommand('insertText', false, '\t');
      if (start === end) input.setSelectionRange(start + 1, start + 1);
      repaint();
      settle();
    }
  });

  document.addEventListener('keydown', function (event) {
    var only = event.ctrlKey && !event.altKey && !event.shiftKey;
    if (only && event.code === 'KeyS') {
      event.preventDefault();
      save();
      return;
    }
    if (only && event.code === 'KeyF') {
      event.preventDefault();
      openFind();
      return;
    }
    if (event.key === 'Escape' && !findbar.hidden) {
      event.preventDefault();
      closeFind();
    }
  });

  findInput.addEventListener('input', function () {
    scan();
    if (hits.length) find(0);
  });

  findInput.addEventListener('keydown', function (event) {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    find(event.shiftKey ? -1 : 1);
  });

  $('mode').onclick = function () {
    setMode(doc.mode === 'table' ? 'text' : 'table');
  };

  /*
   * Changing the encoding re-reads the file.
   *
   * What is on screen is a decoding, not the file, and there is no way to
   * change one without doing the other again. The shell asks about unsaved work
   * before it answers -- it owns the only dialog in the window, and this is the
   * one control here that can throw away an edit.
   */
  $('encoding').onchange = function () {
    var wanted = $('encoding').value;
    if (wanted === doc.encoding) return;
    post('editorReopen', { encoding: wanted });
  };

  $('fontDown').onclick = function () {
    fontSize = Math.max(FONT_MIN, fontSize - 1);
    applyFont();
  };
  $('fontUp').onclick = function () {
    fontSize = Math.min(FONT_MAX, fontSize + 1);
    applyFont();
  };

  $('find').onclick = function () {
    if (findbar.hidden) openFind();
    else closeFind();
  };
  $('findNext').onclick = function () { find(1); };
  $('findPrev').onclick = function () { find(-1); };
  $('findClose').onclick = closeFind;
  saveButton.onclick = save;

  // Clicking the empty space under the last line puts the caret at the end,
  // which is what clicking the empty space under a document does everywhere.
  scroll.addEventListener('mousedown', function (event) {
    if (event.target !== scroll && event.target !== paint) return;
    if (doc.readOnly) return;
    event.preventDefault();
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  });

  window.addEventListener('message', function (event) {
    var message = event.data || {};
    if (message.type === 'editorFile') { show(message.file); return; }
    if (message.type === 'editorMore') { grew(message.chunk || {}); return; }
    if (message.type === 'editorTable') { grewTable(message.chunk || {}); return; }
    if (message.type === 'editorEncoding') {
      // Accepted, so the next save writes what is being read.
      doc.encoding = message.encoding || doc.encoding;
      settle();
      return;
    }
    if (message.type === 'editorSaved') { saved(message); return; }
    if (message.type === 'editorError') { saving = false; say(t(message.reason) || message.reason, true); return; }
  });

  function label() {
    document.querySelectorAll('[data-i18n]').forEach(function (el) {
      el.textContent = S[el.dataset.i18n] || '';
    });
    document.querySelectorAll('[data-i18n-title]').forEach(function (el) {
      el.title = S[el.dataset.i18nTitle] || '';
    });
    document.querySelectorAll('[data-i18n-aria]').forEach(function (el) {
      el.setAttribute('aria-label', S[el.dataset.i18nAria] || '');
    });
    document.querySelectorAll('[data-i18n-placeholder]').forEach(function (el) {
      el.placeholder = S[el.dataset.i18nPlaceholder] || '';
    });
  }

  label();
  loadFont();
  applyFont();
  settle();
  post('editorOpen');
}());
