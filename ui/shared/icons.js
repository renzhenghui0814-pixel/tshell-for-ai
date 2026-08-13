/*
 * The icon set, in one place.
 *
 * Every page already referred to its icons as `<use href="#i-name">` -- the
 * pages carried their own `<symbol>` sprite in their own markup, and chat.js and
 * transfer.js build the same references as strings at runtime. Two pages meant
 * two sprites, which meant two drawings of a pencil and two of a bin, and they
 * had already drifted: the title bar's glyphs were 1.2px outlines while the file
 * panel's were solid fills.
 *
 * This file is the single sprite. It is injected into whatever page loads it, so
 * every `<use href="#i-...">` that already existed resolves to the same drawing,
 * including the ones JavaScript writes. No page had to change to get here --
 * same trick as the `--vscode-*` aliases in theme.css: keep the interface, and
 * replace what is behind it.
 *
 * THE SYSTEM
 *
 *   16x16 viewBox, shapes inside a 12.5px optical box, 1.4px stroke, round caps
 *   and joins, no fill. Curves are 1.3-2.2px radii; nothing is a hard right
 *   angle except where the object genuinely has one.
 *
 * Outlines and not fills, because these are drawn at 13-16px and a filled glyph
 * at that size is a blob with a silhouette -- the counters are what carry the
 * meaning. Three icons opt out and say why at their definition.
 *
 * The stroke attributes go on a `<g>` inside each symbol rather than in CSS.
 * Pages set `.icon { fill: currentColor }`, which inherits into the shadow tree
 * a `<use>` builds; a presentation attribute on the group is a declaration *on*
 * that element and so beats the inherited value. That means these render
 * correctly in a page whose stylesheet has not been touched.
 */
(function () {
  'use strict';

  /*
   * Window controls are deliberately absent. Minimise, maximise and close are
   * system furniture, not product iconography: they are 1px crisp lines at the
   * metrics Windows uses, and drawing them in this set's round-capped 1.4px
   * would make the title bar read as a web page imitating a title bar. They
   * stay inline in index.html.
   */
  var ICONS = {

    /* -- chrome ------------------------------------------------------------ */

    /* A pane with a prompt in it. The chevron is the shape people read as
     * "terminal" faster than any amount of window. */
    'terminal':
      '<rect x="1.9" y="2.7" width="12.2" height="10.6" rx="2.2"/>' +
      '<path d="M4.9 6.5l1.9 1.7-1.9 1.7M8.7 10.4h2.9"/>',

    /* Two arrows passing: the panel moves files both ways, and neither
     * direction is the primary one. */
    'transfer':
      '<path d="M2.7 5.8h8.5M8.9 3.5l2.3 2.3-2.3 2.3"/>' +
      '<path d="M13.3 10.2H4.8M7.1 7.9L4.8 10.2l2.3 2.3"/>',

    /*
     * The assistant's mark: two four-pointed stars, a large one on the left and
     * a small one off its bottom-right corner.
     *
     * The big one is outlined like everything else in the set; only the small
     * one is solid. A four-pointed star is concave -- its edge curves back in
     * towards the middle between every pair of points -- so an outline of it
     * only survives if the waist stays wider than two strokes. At r=4.6 with
     * the curve controls pushed a fifth of the radius out from the centre the
     * waist is 4.3px across, which leaves a 2.9px counter: open at every size
     * this is drawn. The small star has no such room -- 1.1px of waist would
     * close up entirely -- so it stays filled, and the pair reads as one
     * outline plus one dot rather than two of the same thing.
     *
     * Orange, and specifically the orange of the application icon beside it on
     * the title bar. The assistant is the product's own headline feature and
     * the two marks are three characters apart up there; they should read as
     * one family. It is the one glyph in the set that does not take
     * `currentColor` from whatever is around it -- see `--ai` in theme.css.
     */
    'ai':
      '<path d="M6.4 2.1Q7.05 6.05 11 6.7 7.05 7.35 6.4 11.3 5.75 7.35 1.8 6.7 5.75 6.05 6.4 2.1Z"/>' +
      '<path fill="currentColor" stroke="none" d="M11.8 9.2Q12.14 11.26 14.2 11.6 12.14 11.94 11.8 14 11.46 11.94 9.4 11.6 11.46 11.26 11.8 9.2Z"/>',

    /*
     * A window with a rail down the left, in two states: with content in the
     * rail and without.
     *
     * Two icons rather than one icon and a CSS rule. The old sprite-less
     * version hid the three dashes with `display: none` on a class inside the
     * glyph, which stops working the moment the glyph arrives through a
     * `<use>`: that content lives in a shadow tree, and the only thing document
     * CSS reaches into a shadow tree with is inherited properties. `display` is
     * not one. Swapping the href is the version that works.
     */
    'panel':
      '<rect x="1.8" y="2.8" width="12.4" height="10.4" rx="2"/>' +
      '<path d="M6.2 2.8v10.4"/>' +
      '<path d="M3.4 5.6h1.4M3.4 8h1.4M3.4 10.4h1.4"/>',

    'panel-off':
      '<rect x="1.8" y="2.8" width="12.4" height="10.4" rx="2"/>' +
      '<path d="M6.2 2.8v10.4"/>',

    /*
     * One outline traced around eight teeth, plus the bore.
     *
     * Generated rather than drawn: every tooth is the same four moves at
     * 45-degree intervals -- tip arc at r=6.2, flank in to r=4.5, root arc,
     * flank back out -- so all eight are identical to the hundredth of a unit.
     * The hand-fitted version this replaces was built from rounded-off line
     * deltas and the error accumulated around the rim; by the third quadrant
     * the teeth were visibly different heights and widths from the first.
     *
     * The teeth are narrow (14 degrees) and the valleys between them wide (28).
     * That is backwards from a real gear and deliberate. At 1.4px of stroke the
     * only thing that can close up is a gap, and a valley narrower than about
     * 2.2px at the root fills solid -- which is what turns a gear into a
     * cog-shaped blob. A tooth whose own counter closes is no loss: it reads as
     * a bump on the rim, which is all a tooth has to be at 15px. So the gaps
     * get the width and the teeth get whatever is left.
     */
    'gear':
      '<path d="M7.24 1.85A6.2 6.2 0 0 1 8.76 1.85L8.67 3.55A4.5 4.5 0 0 1 10.68 4.38L11.82 3.11A6.2 6.2 0 0 1 12.89 4.18L11.62 5.32A4.5 4.5 0 0 1 12.45 7.33L14.15 7.24A6.2 6.2 0 0 1 14.15 8.76L12.45 8.67A4.5 4.5 0 0 1 11.62 10.68L12.89 11.82A6.2 6.2 0 0 1 11.82 12.89L10.68 11.62A4.5 4.5 0 0 1 8.67 12.45L8.76 14.15A6.2 6.2 0 0 1 7.24 14.15L7.33 12.45A4.5 4.5 0 0 1 5.32 11.62L4.18 12.89A6.2 6.2 0 0 1 3.11 11.82L4.38 10.68A4.5 4.5 0 0 1 3.55 8.67L1.85 8.76A6.2 6.2 0 0 1 1.85 7.24L3.55 7.33A4.5 4.5 0 0 1 4.38 5.32L3.11 4.18A6.2 6.2 0 0 1 4.18 3.11L5.32 4.38A4.5 4.5 0 0 1 7.33 3.55Z"/>' +
      '<circle cx="8" cy="8" r="2.05"/>',

    'search':
      '<circle cx="7.2" cy="7.2" r="4.3"/>' +
      '<path d="M10.5 10.5l3 3"/>',

    'plus': '<path d="M8 3.2v9.6M3.2 8h9.6"/>',

    'close': '<path d="M4.2 4.2l7.6 7.6M11.8 4.2l-7.6 7.6"/>',

    'check': '<path d="M3.2 8.4l3.2 3.2 6.4-7.2"/>',

    /*
     * FILLED, on purpose. This is the sort indicator in the file panel's column
     * head, and it is drawn at 8px. A 1.4px outline at 8px is a triangle whose
     * hole is a third of a pixel: it renders as a smudge. A solid arrowhead is
     * the one shape that survives that size.
     */
    'caret': '<path fill="currentColor" stroke="none" d="M8 5.4l4.2 5.2H3.8z"/>',

    'chevron': '<path d="M6.2 3.6L10.6 8l-4.4 4.4"/>',

    /* Stacked units with a lamp on each. For groups in the server tree. */
    'server':
      '<rect x="2.2" y="2.6" width="11.6" height="4.4" rx="1.4"/>' +
      '<rect x="2.2" y="9" width="11.6" height="4.4" rx="1.4"/>' +
      '<circle cx="4.9" cy="4.8" r=".75" fill="currentColor" stroke="none"/>' +
      '<circle cx="4.9" cy="11.2" r=".75" fill="currentColor" stroke="none"/>',

    /* -- files ------------------------------------------------------------- */

    'folder':
      '<path d="M1.9 4.1a1.3 1.3 0 0 1 1.3-1.3h2.6l1.5 1.9h5.6a1.3 1.3 0 0 1 1.3 1.3v6.1a1.3 1.3 0 0 1-1.3 1.3H3.2a1.3 1.3 0 0 1-1.3-1.3z"/>',

    'file':
      '<path d="M9.2 2.2H4.9a1.3 1.3 0 0 0-1.3 1.3v9a1.3 1.3 0 0 0 1.3 1.3h6.2a1.3 1.3 0 0 0 1.3-1.3V5.2z"/>' +
      '<path d="M9.2 2.2v3h3"/>',

    /* A file with the shortcut arrow the object itself carries. */
    'link':
      '<path d="M9.2 2.2H4.9a1.3 1.3 0 0 0-1.3 1.3v9a1.3 1.3 0 0 0 1.3 1.3h6.2a1.3 1.3 0 0 0 1.3-1.3V5.2z"/>' +
      '<path d="M9.2 2.2v3h3"/>' +
      '<path d="M6 11.1l3.3-3.3M6.9 7.8h2.4v2.4"/>',

    'new-folder':
      '<path d="M1.9 4.1a1.3 1.3 0 0 1 1.3-1.3h2.6l1.5 1.9h5.6a1.3 1.3 0 0 1 1.3 1.3v6.1a1.3 1.3 0 0 1-1.3 1.3H3.2a1.3 1.3 0 0 1-1.3-1.3z"/>' +
      '<path d="M8 7v4.1M5.95 9.05h4.1"/>',

    'up': '<path d="M8 12.9V3.5M4.2 7.3L8 3.5l3.8 3.8"/>',

    'eye':
      '<path d="M1.4 8S3.9 3.7 8 3.7 14.6 8 14.6 8 12.1 12.3 8 12.3 1.4 8 1.4 8z"/>' +
      '<circle cx="8" cy="8" r="2.1"/>',

    'download':
      '<path d="M8 2.6v7.5M4.9 7l3.1 3.1L11.1 7"/>' +
      '<path d="M2.8 11.9v1a1.2 1.2 0 0 0 1.2 1.2h8a1.2 1.2 0 0 0 1.2-1.2v-1"/>',

    'upload':
      '<path d="M8 10.1V2.6M4.9 5.7L8 2.6l3.1 3.1"/>' +
      '<path d="M2.8 11.9v1a1.2 1.2 0 0 0 1.2 1.2h8a1.2 1.2 0 0 0 1.2-1.2v-1"/>',

    'copy':
      '<rect x="5.6" y="5.6" width="8.2" height="8.2" rx="1.8"/>' +
      '<path d="M11.1 5.6V4a1.8 1.8 0 0 0-1.8-1.8H4A1.8 1.8 0 0 0 2.2 4v5.3A1.8 1.8 0 0 0 4 11.1h1.6"/>',

    'edit':
      '<path d="M11.7 1.9a1.4 1.4 0 0 1 2 2l-8 8-2.9.9.9-2.9z"/>' +
      '<path d="M10.7 2.9l2 2"/>',

    'trash':
      '<path d="M2.9 4.3h10.2"/>' +
      '<path d="M6.4 4.3V3.1a.9.9 0 0 1 .9-.9h1.4a.9.9 0 0 1 .9.9v1.2"/>' +
      '<path d="M4.3 4.3l.6 8.5a1 1 0 0 0 1 .9h4.2a1 1 0 0 0 1-.9l.6-8.5"/>' +
      '<path d="M6.8 6.8v4.2M9.2 6.8v4.2"/>',

    /* Two arcs and two heads. One arc with one head is the commoner drawing and
     * it is ambiguous at this size -- it reads as a partial circle. */
    'refresh':
      '<path d="M2.7 8a5.3 5.3 0 0 1 9.05-3.75L13.3 5.8"/>' +
      '<path d="M13.3 2.3v3.5h-3.5"/>' +
      '<path d="M13.3 8a5.3 5.3 0 0 1-9.05 3.75L2.7 10.2"/>' +
      '<path d="M2.7 13.7v-3.5h3.5"/>',

    'lock':
      '<rect x="3.4" y="7.1" width="9.2" height="6.5" rx="1.7"/>' +
      '<path d="M5.7 7.1V5.3a2.3 2.3 0 0 1 4.6 0v1.8"/>',

    'key':
      '<circle cx="5.6" cy="10.4" r="2.8"/>' +
      '<path d="M7.6 8.4l5.5-5.5M11.2 4.8l1.4 1.4M9.7 6.3l1.4 1.4"/>',

    /* -- the assistant ----------------------------------------------------- */

    /*
     * FILLED, on purpose. Every other icon here names a thing; this one is a
     * state -- the run is going, and pressing it kills the run. Outlined, it
     * reads as "a square"; solid, it reads as the stop on every transport
     * control ever made, which is the association worth having on the one
     * control in the window that interrupts work in flight.
     */
    'stop': '<rect x="4.2" y="4.2" width="7.6" height="7.6" rx="1.6" fill="currentColor" stroke="none"/>',

    'send': '<path d="M8 13.2V3.2M3.9 7.3L8 3.2l4.1 4.1"/>',

    'history':
      '<path d="M2.6 8a5.4 5.4 0 1 0 1.55-3.8L2.4 5.85"/>' +
      '<path d="M2.2 2.5v3.4h3.4"/>' +
      '<path d="M8 5.1V8l2.2 1.35"/>',

    /* A bookmark: what has been kept, rather than a chip or a brain. */
    'memory':
      '<path d="M4.2 2.9a.9.9 0 0 1 .9-.9h5.8a.9.9 0 0 1 .9.9v10.5L8 10.9l-3.8 2.5z"/>',

    /* A spanner, for the procedures someone wrote for this setup. It sits
     * between what the assistant remembers by itself and what it is thinking,
     * which is where it belongs: knowledge, but the kind a person put there. */
    'skill':
      '<path d="M10.4 2.35a3.5 3.5 0 0 0-2.5 5.3l-5.2 5.2a1.45 1.45 0 0 0 2.05 2.05l5.2-5.2a3.5 3.5 0 0 0 4.25-4.45l-1.85 1.85-1.7-.45-.45-1.7 1.85-1.85a3.5 3.5 0 0 0-1.65-.75z"/>',

    /* Three equal rings, overlapping. The only thing in the assistant's row
     * that is about seeing rather than doing: the model's own thinking, shown
     * or not shown. What makes it legible at 14px is the holes, not the
     * silhouette -- a brain and a solid cloud both stood here and both read as
     * a lump. */
    'think':
      '<circle cx="5.2" cy="9.6" r="3.2"/>' +
      '<circle cx="10.8" cy="9.6" r="3.2"/>' +
      '<circle cx="8" cy="6.2" r="3.2"/>',

    /*
     * The three modes, as three different shapes rather than one shape in three
     * colours. Which mode is on decides whether anything gets to ask before it
     * happens, so it has to be readable at a glance, out of the corner of an
     * eye, and by someone who cannot tell the colours apart. Colour is carried
     * too -- auto goes red -- but only ever as the second thing saying it.
     */
    'mode-ask':
      '<path d="M5.4 5.6A2.7 2.7 0 1 1 8 8.4v1.5"/>' +
      '<circle cx="8" cy="12.6" r="1" fill="currentColor" stroke="none"/>',

    'mode-trust':
      '<path d="M8 2.3l4.7 1.8v3.95c0 2.7-1.9 4.7-4.7 5.4-2.8-.7-4.7-2.7-4.7-5.4V4.1z"/>' +
      '<path d="M5.85 7.9l1.6 1.6 2.95-3.2"/>',

    'mode-auto': '<path d="M9.4 2l-5.6 7h3.4l-.6 5 5.6-7H8.8z"/>',

    /* A checked-off list. The shield above is the policy; this is what it
     * reads. Two rows rather than three -- at this size a third only fills the
     * gaps that make the first two legible. */
    'trust':
      '<path d="M2.2 5.1l1.35 1.4 2.65-3M2.2 11.1l1.35 1.4 2.65-3"/>' +
      '<path d="M8.7 5.1h5.1M8.7 11.1h5.1"/>',

    'warn':
      '<path d="M8 2.6l5.8 10.2H2.2z"/>' +
      '<path d="M8 6.5v3"/>' +
      '<circle cx="8" cy="11.3" r=".75" fill="currentColor" stroke="none"/>'
  };

  var NS = 'http://www.w3.org/2000/svg';

  function buildSprite() {
    var parts = [];
    for (var name in ICONS) {
      if (!Object.prototype.hasOwnProperty.call(ICONS, name)) continue;
      parts.push(
        '<symbol id="i-' + name + '" viewBox="0 0 16 16">' +
          '<g fill="none" stroke="currentColor" stroke-width="1.4" ' +
             'stroke-linecap="round" stroke-linejoin="round">' +
            ICONS[name] +
          '</g>' +
        '</symbol>'
      );
    }

    var sprite = document.createElementNS(NS, 'svg');
    sprite.setAttribute('width', '0');
    sprite.setAttribute('height', '0');
    sprite.setAttribute('aria-hidden', 'true');
    sprite.setAttribute('focusable', 'false');
    sprite.setAttribute('id', 'tshell-icon-sprite');
    sprite.style.position = 'absolute';
    sprite.innerHTML = parts.join('');
    return sprite;
  }

  function install() {
    if (document.getElementById('tshell-icon-sprite')) return;
    // First child, so a `<use>` further down the document finds its target
    // already parsed rather than relying on a later re-resolve.
    document.body.insertBefore(buildSprite(), document.body.firstChild);
  }

  if (document.body) install();
  else document.addEventListener('DOMContentLoaded', install);

  /*
   * For the shell, which builds tab strips and status bars from script and needs
   * a whole element rather than a reference into the sprite.
   */
  window.tshellIcon = function (name, className) {
    var svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('class', className || 'icon');
    svg.setAttribute('viewBox', '0 0 16 16');
    svg.setAttribute('aria-hidden', 'true');
    var use = document.createElementNS(NS, 'use');
    use.setAttribute('href', '#i-' + name);
    svg.appendChild(use);
    return svg;
  };

  window.tshellIconNames = Object.keys(ICONS);

  /*
   * The product's own mark, in blocks.
   *
   * media/tshell.svg draws the nautilus whorl as one filled path with a tapered
   * stroke -- 1.85 turns of r = e^(0.30 theta), thickening from 3% of the outer
   * radius at the centre to 34% at the mouth. This is that same curve sampled
   * onto a 17x15 grid: a cell is set if the stroke covers more than a third of
   * it. Nothing here was placed by hand, which is why the bitmap below can be
   * read as a picture in the source and still be the logo.
   *
   * Blocks, and not the curve itself, because this is the empty state of the
   * assistant panel and it is the only place in the window where the mark is
   * decoration rather than identity. Drawn smooth at 100px next to a paragraph
   * of help text it reads as a watermark someone forgot to remove; drawn as
   * something assembled out of parts, it reads as a thing waiting to be built,
   * which is what an empty conversation is.
   *
   * The blocks stay square with a 0.6-unit gutter at every size. That gutter is
   * the whole effect -- close it and this is just a low-resolution logo.
   */
  var MARK = [
    '.......####......',
    '.....#########...',
    '...#############.',
    '..###############',
    '.#####......#####',
    '.####.........##.',
    '####.............',
    '####.............',
    '###...###........',
    '###..#####.......',
    '####..####.......',
    '.####..###.......',
    '..########.......',
    '..#######........',
    '....###..........'
  ];

  window.tshellMark = function (className) {
    var CELL = 4, BLOCK = 3.4, INSET = (CELL - BLOCK) / 2, RADIUS = 0.9;
    var svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('class', className || 'mark');
    svg.setAttribute('viewBox', '0 0 ' + MARK[0].length * CELL + ' ' + MARK.length * CELL);
    svg.setAttribute('fill', 'currentColor');
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('focusable', 'false');
    for (var y = 0; y < MARK.length; y++) {
      for (var x = 0; x < MARK[y].length; x++) {
        if (MARK[y].charAt(x) !== '#') continue;
        var block = document.createElementNS(NS, 'rect');
        block.setAttribute('x', x * CELL + INSET);
        block.setAttribute('y', y * CELL + INSET);
        block.setAttribute('width', BLOCK);
        block.setAttribute('height', BLOCK);
        block.setAttribute('rx', RADIUS);
        svg.appendChild(block);
      }
    }
    return svg;
  };
})();
