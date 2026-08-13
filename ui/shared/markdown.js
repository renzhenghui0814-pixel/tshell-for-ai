/*
 * The markdown the assistant actually writes, and nothing else.
 *
 * Models reply in markdown whether or not you ask them to, so the panel has to
 * read it. This covers the shapes that turn up in practice -- headings, lists,
 * emphasis, inline code and fenced code -- and deliberately stops there: no
 * tables, links or images, because none of them survive a narrow side panel and
 * every one of them is more parser to get wrong.
 *
 * The split is the point. parseBlocks() is pure text in, plain objects out, with
 * no DOM anywhere near it. render() is the thin layer that turns that tree into
 * nodes. Model output reaches the document as textContent and never as markup;
 * the single exception is a highlighted code line, whose HTML the shared
 * highlighter built out of escaped pieces.
 */
(function () {
  'use strict';

  const FENCE = /^\s*(?:```|~~~)\s*([\w+#.-]*)\s*$/;
  const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
  const HR = /^\s{0,3}(?:-{3,}|\*{3,}|_{3,})\s*$/;
  const QUOTE = /^\s{0,3}>\s?(.*)$/;
  const BULLET = /^(\s*)[-*+]\s+(.*)$/;
  const ORDERED = /^(\s*)\d+[.)]\s+(.*)$/;

  /*
   * Emphasis is asterisks only. Underscores are left alone on purpose: this panel
   * is full of snake_case identifiers and $SOME_VAR, and turning half of one into
   * italics is a worse failure than not rendering _italic_ at all.
   *
   * Both forms require the run to begin and end on a non-space, which is what
   * keeps a glob such as `ls *.log and *.txt` from reading as emphasis.
   */
  const INLINE = /`([^`]+)`|\*\*(\S(?:[^*]*\S)?)\*\*|\*(\S(?:[^*]*\S)?)\*/;

  /*
   * A table is recognised by its second line, not its first: the header row is
   * ordinary text that happens to contain pipes, and only the dashed row below it
   * settles the matter. Everything here therefore looks one line ahead.
   */
  const TABLE_DELIM = /^[\s|:-]*-[\s|:-]*$/;

  function isDelimiterRow(line) {
    return line.includes('-') && line.includes('|') && TABLE_DELIM.test(line);
  }

  function isTableStart(lines, index) {
    return lines[index].includes('|') && index + 1 < lines.length && isDelimiterRow(lines[index + 1]);
  }

  /*
   * Outer pipes are optional in the wild, so they are trimmed before splitting.
   * A pipe inside a cell's inline code still ends the cell -- escaping it properly
   * costs a tokenizer, and a model writing `a|b` in a table is rarer than a model
   * writing a table at all.
   */
  function splitRow(line) {
    let row = line.trim();
    if (row.startsWith('|')) row = row.slice(1);
    if (row.endsWith('|')) row = row.slice(0, -1);
    return row.split('|').map((cell) => cell.trim());
  }

  function alignOf(spec) {
    const cell = spec.trim();
    if (cell.startsWith(':') && cell.endsWith(':')) return 'center';
    if (cell.endsWith(':')) return 'right';
    return '';
  }

  function isBlockStart(line) {
    return !line.trim() || FENCE.test(line) || HEADING.test(line) || HR.test(line)
      || QUOTE.test(line) || BULLET.test(line) || ORDERED.test(line);
  }

  function parseInline(text) {
    const nodes = [];
    let rest = String(text);
    for (;;) {
      const match = rest.match(INLINE);
      if (!match) break;
      if (match.index > 0) nodes.push({ type: 'text', text: rest.slice(0, match.index) });
      // Code wins over emphasis, so `**not bold**` in backticks stays literal.
      if (match[1] !== undefined) nodes.push({ type: 'code', text: match[1] });
      else if (match[2] !== undefined) nodes.push({ type: 'strong', children: parseInline(match[2]) });
      else nodes.push({ type: 'em', children: parseInline(match[3]) });
      rest = rest.slice(match.index + match[0].length);
    }
    if (rest) nodes.push({ type: 'text', text: rest });
    return nodes;
  }

  /*
   * Collects a run of list rows, then hangs anything indented under the item above
   * it. Only one level of nesting is kept; deeper ones flatten into it, because a
   * side panel runs out of width long before a model runs out of nesting.
   */
  function buildList(rows) {
    const list = { type: 'list', ordered: rows[0].ordered, items: [] };
    for (const row of rows) {
      const parent = list.items[list.items.length - 1];
      if (row.indent >= 2 && parent) {
        if (!parent.child) parent.child = { type: 'list', ordered: row.ordered, items: [] };
        parent.child.items.push({ text: row.text });
      } else {
        list.items.push({ text: row.text });
      }
    }
    return list;
  }

  function parseBlocks(source) {
    const lines = String(source === undefined || source === null ? '' : source).replace(/\r\n?/g, '\n').split('\n');
    const blocks = [];
    let i = 0;

    while (i < lines.length) {
      const line = lines[i];

      if (!line.trim()) { i += 1; continue; }

      const fence = line.match(FENCE);
      if (fence) {
        const body = [];
        i += 1;
        while (i < lines.length && !FENCE.test(lines[i])) { body.push(lines[i]); i += 1; }
        // An unclosed fence runs to the end of the reply. Showing the rest as code
        // is wrong in a small way; dropping it is wrong in a large one.
        i += 1;
        blocks.push({ type: 'code', lang: fence[1].toLowerCase(), lines: body });
        continue;
      }

      // Checked before the bullet rule, which would otherwise claim `* * *`.
      if (HR.test(line)) { blocks.push({ type: 'hr' }); i += 1; continue; }

      const heading = line.match(HEADING);
      if (heading) {
        blocks.push({ type: 'heading', level: heading[1].length, text: heading[2] });
        i += 1;
        continue;
      }

      const quote = line.match(QUOTE);
      if (quote) {
        const body = [quote[1]];
        i += 1;
        while (i < lines.length && QUOTE.test(lines[i])) { body.push(lines[i].match(QUOTE)[1]); i += 1; }
        blocks.push({ type: 'quote', text: body.join('\n') });
        continue;
      }

      if (BULLET.test(line) || ORDERED.test(line)) {
        const rows = [];
        while (i < lines.length) {
          const ordered = lines[i].match(ORDERED);
          const match = ordered || lines[i].match(BULLET);
          if (!match) break;
          rows.push({ indent: match[1].length, text: match[2], ordered: Boolean(ordered) });
          i += 1;
        }
        blocks.push(buildList(rows));
        continue;
      }

      if (isTableStart(lines, i)) {
        const header = splitRow(line);
        const align = splitRow(lines[i + 1]).map(alignOf);
        const rows = [];
        i += 2;
        while (i < lines.length && lines[i].trim() && lines[i].includes('|')) {
          rows.push(splitRow(lines[i]));
          i += 1;
        }
        blocks.push({ type: 'table', align: align, header: header, rows: rows });
        continue;
      }

      const paragraph = [line];
      i += 1;
      while (i < lines.length && !isBlockStart(lines[i]) && !isTableStart(lines, i)) { paragraph.push(lines[i]); i += 1; }
      // Joined with the newlines intact: in a chat reply a line break is usually
      // meant, unlike in a document where markdown would reflow it away.
      blocks.push({ type: 'para', text: paragraph.join('\n') });
    }

    return blocks;
  }

  // -- rendering ------------------------------------------------------------

  function renderInline(nodes, parent) {
    for (const node of nodes) {
      if (node.type === 'text') { parent.append(document.createTextNode(node.text)); continue; }
      if (node.type === 'code') {
        const code = document.createElement('code');
        code.className = 'md-code';
        code.textContent = node.text;
        parent.append(code);
        continue;
      }
      const el = document.createElement(node.type === 'strong' ? 'strong' : 'em');
      renderInline(node.children, el);
      parent.append(el);
    }
  }

  function inlineInto(el, text) {
    renderInline(parseInline(text), el);
    return el;
  }

  function element(tag, className) {
    const el = document.createElement(tag);
    if (className) el.className = className;
    return el;
  }

  function fallbackCopy(text, done) {
    const area = document.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.append(area);
    area.select();
    try { document.execCommand('copy'); done(); } finally { area.remove(); }
  }

  function copyText(text, button, strings) {
    const done = () => {
      button.textContent = strings.copied;
      button.classList.add('done');
      setTimeout(() => { button.textContent = strings.copy; button.classList.remove('done'); }, 1400);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, () => fallbackCopy(text, done));
      return;
    }
    fallbackCopy(text, done);
  }

  function renderCode(block, strings, options) {
    const wrap = element('div', 'md-block');
    const head = element('div', 'md-block-head');
    const label = element('span', 'md-block-lang');
    label.textContent = block.lang || '';

    const copy = element('button', 'md-copy');
    copy.type = 'button';
    copy.textContent = strings.copy;
    const text = block.lines.join('\n');
    copy.onclick = () => copyText(text, copy, strings);
    head.append(label, copy);

    const pre = element('pre', 'md-pre');
    const code = document.createElement('code');
    // The tag when there is a usable one, the content's own evidence otherwise:
    // an untagged fence is still code, and models leave the tag off often enough
    // that "no tag" cannot mean "no colour". Unless the caller asked for none at
    // all, which is what `highlight: false` is: see render().
    const language = options && options.highlight === false
      ? ''
      : window.tshellHighlight.normalizeLanguage(block.lang) || window.tshellHighlight.guessLanguage(text);
    if (language) {
      // The one place model text becomes markup. highlightText escapes every
      // literal it emits, so what lands here is spans around escaped content.
      code.innerHTML = window.tshellHighlight.highlightText(text, language);
    } else {
      code.textContent = text;
    }

    /*
     * Numbers live in a column of their own rather than at the head of each line,
     * which is what keeps them out of a copied selection and still on screen once
     * a long line has been scrolled sideways: only the code beside them scrolls.
     */
    const gutter = element('div', 'md-gutter');
    gutter.setAttribute('aria-hidden', 'true');
    gutter.textContent = block.lines.map((line, index) => String(index + 1)).join('\n');

    const scroll = element('div', 'md-scroll');
    scroll.append(code);
    pre.append(gutter, scroll);
    wrap.append(head, pre);
    return wrap;
  }

  function cell(tag, text, align) {
    const el = inlineInto(document.createElement(tag), text);
    // Set through a class rather than a style attribute, so the panel's content
    // security policy is never the thing standing between a column and its
    // alignment.
    if (align) el.className = 'md-al-' + align;
    return el;
  }

  /*
   * Wrapped in its own scroller. A table is the one block that will not narrow to
   * fit a side panel, and letting it push the panel wide would break every other
   * message in the thread.
   */
  function renderTable(block) {
    const wrap = element('div', 'md-table-wrap');
    const table = element('table', 'md-table');
    const head = document.createElement('tr');
    block.header.forEach((text, index) => head.append(cell('th', text, block.align[index])));

    const body = document.createElement('tbody');
    for (const row of block.rows) {
      const tr = document.createElement('tr');
      // Driven by the header's width, so a row with too few or too many cells
      // lines up with the columns instead of skewing the whole table.
      for (let index = 0; index < block.header.length; index += 1) {
        tr.append(cell('td', row[index] === undefined ? '' : row[index], block.align[index]));
      }
      body.append(tr);
    }

    const header = document.createElement('thead');
    header.append(head);
    table.append(header, body);
    wrap.append(table);
    return wrap;
  }

  function renderBlock(block, strings, options) {
    switch (block.type) {
      case 'code':
        return renderCode(block, strings, options);
      case 'table':
        return renderTable(block);
      case 'hr':
        return element('hr', 'md-hr');
      case 'heading':
        return inlineInto(element('h' + Math.min(block.level, 6), 'md-h md-h' + block.level), block.text);
      case 'quote':
        return inlineInto(element('blockquote', 'md-quote'), block.text);
      case 'list': {
        const list = element(block.ordered ? 'ol' : 'ul', 'md-list');
        for (const item of block.items) {
          const li = inlineInto(document.createElement('li'), item.text);
          if (item.child) li.append(renderBlock(item.child, strings, options));
          list.append(li);
        }
        return list;
      }
      default:
        return inlineInto(element('p', 'md-p'), block.text);
    }
  }

  /**
   * @param options `{ highlight: false }` renders code fences as plain text.
   *   For text that is not an answer -- a model's own thinking, which is a
   *   draft of code as often as it is code, and which colouring dresses up as
   *   something more finished than it is.
   */
  function render(source, strings, options) {
    const labels = strings || {};
    const fragment = document.createDocumentFragment();
    try {
      for (const block of parseBlocks(source)) fragment.append(renderBlock(block, labels, options));
    } catch (error) {
      // A formatting bug must never cost the user the reply itself.
      fragment.replaceChildren(document.createTextNode(String(source === undefined || source === null ? '' : source)));
    }
    return fragment;
  }

  window.tshellMarkdown = { parseBlocks: parseBlocks, parseInline: parseInline, render: render };
}());
