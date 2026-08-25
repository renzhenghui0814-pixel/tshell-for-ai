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

    /*
     * -- file kinds -----------------------------------------------------------
     *
     * THE FOURTH OPT-OUT, and the largest. These are solid, and the rule at the
     * top of this file says outlines.
     *
     * The rule is about a glyph read on its own -- a toolbar button, a menu row
     * -- where 1.4px of outline round a lot of white is what keeps a 15px shape
     * from being a smudge. These are not read on their own. They are read forty
     * at a time down the left edge of a file list, at a glance, mostly in
     * peripheral vision, and what the eye is doing there is not identifying a
     * drawing but sorting a column into kinds. A solid shape in a distinct hue
     * does that at a smaller size than an outline can, which is why every file
     * manager that has ever shipped draws them this way.
     *
     * The counters still carry the meaning, which is why almost all of these
     * are `fill-rule="evenodd"`: the marks are knocked *out* of the solid, so
     * what identifies a JSON file from six feet away is the two gaps across the
     * cylinder rather than any edge of it.
     *
     * Before this, every file in the list was the same outlined page and only
     * its colour changed -- so the icon column said "file, file, file, file" in
     * eight hues, and the shape, which is the channel the eye reads first, was
     * carrying nothing at all.
     *
     * The colours are not here. They are `.ic-*` in transfer.css, on the
     * syntax-highlighting set, because they are exactly that job: a handful of
     * hues whose only requirement is to stay apart from each other.
     */

    /* A folder. The one shape in the set nobody has to be taught. */
    'file-dir':
      '<path fill="currentColor" stroke="none" d="M2.4 4.3a1.2 1.2 0 0 1 1.2-1.2h3.1' +
      'l1.7 1.8h4.4a1.2 1.2 0 0 1 1.2 1.2v6.6a1.2 1.2 0 0 1-1.2 1.2H3.6' +
      'a1.2 1.2 0 0 1-1.2-1.2z"/>',

    /* Nothing known about it. A page, and no mark -- the absence is the
     * statement, and it is the one every unrecognised extension gets. */
    'file-plain':
      '<path fill="currentColor" stroke="none" d="M4.5 1.8h4.3l3.8 3.8v7.5a1.1 1.1 0 0 1-1.1 1.1H4.5a1.1 1.1 0 0 1-1.1-1.1V2.9a1.1 1.1 0 0 1 1.1-1.1z"/>',

    /* Something written to be read: text, markdown, a log, a PDF. Three lines
     * knocked out of the page, the last one short, which is what a paragraph
     * looks like from far enough away to not be reading it. */
    'file-doc':
      '<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M4.5 1.8h4.3l3.8 3.8v7.5a1.1 1.1 0 0 1-1.1 1.1H4.5a1.1 1.1 0 0 1-1.1-1.1V2.9a1.1 1.1 0 0 1 1.1-1.1z' +
      'M5.5 7.0h5.0v1.1H5.5zM5.5 9.3h5.0v1.1H5.5zM5.5 11.6h3.2v1.1H5.5z"/>',

    /* Source. Two chevrons, which is what a programmer's eye has been trained
     * on for thirty years; no page behind them, because the page is the part
     * that is the same for everything and the chevrons are the part that is
     * not. */
    'file-code':
      '<path fill="currentColor" stroke="none" d="M6.3 3.7 7.4 4.8 4.2 8l3.2 3.2' +
      '-1.1 1.1L2.0 8zM9.7 3.7 8.6 4.8 11.8 8l-3.2 3.2 1.1 1.1L14.0 8z"/>',

    /* Markup: HTML, CSS, XML, SVG. The same chevrons with a slash between them,
     * which is the mark the whole web uses for itself. Deliberately close to
     * `file-code` -- these two kinds ARE close, and pretending otherwise by
     * giving markup an unrelated shape would be inventing a distinction the
     * files do not have. The hue is the second thing telling them apart. */
    'file-markup':
      '<path fill="currentColor" stroke="none" d="M5.0 4.3 5.95 5.25 3.4 8l2.55 2.75' +
      '-0.95 0.95L1.9 8zM11.0 4.3 10.05 5.25 12.6 8l-2.55 2.75 0.95 0.95L14.1 8z' +
      'M8.55 3.5h1.25L7.45 12.5H6.2z"/>',

    /* Structured data: JSON, YAML, CSV, SQL, a DBF. A stack of discs -- the
     * drum every database has been drawn as since the tape era, and the only
     * shape here that says "rows" rather than "words". */
    'file-data':
      '<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M8 2.3' +
      'c2.54 0 4.6.85 4.6 1.9v7.6c0 1.05-2.06 1.9-4.6 1.9s-4.6-.85-4.6-1.9V4.2' +
      'c0-1.05 2.06-1.9 4.6-1.9zM3.4 6.0h9.2v0.9H3.4zM3.4 8.9h9.2v0.9H3.4z"/>',

    /* An archive. A crate: the seam of the lid across it and a catch below,
     * both knocked out. Not a zip pull -- half of what lands in here is a
     * tarball, and nothing in a tarball has ever had a zip on it. */
    'file-archive':
      '<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M3.9 3.6h8.2' +
      'a1.3 1.3 0 0 1 1.3 1.3v7.8a1.3 1.3 0 0 1-1.3 1.3H3.9a1.3 1.3 0 0 1-1.3-1.3' +
      'V4.9a1.3 1.3 0 0 1 1.3-1.3zM2.6 6.9h10.8v1.0H2.6zM7.0 8.6h2.0v2.2H7.0z"/>',

    /* Compiled: a .so, a .dll, an .exe, an .o. A chip, pins and all. It is the
     * one kind in the list that is not for a person to read, and a shape with
     * legs on it says machine before any of the others do. */
    'file-binary':
      '<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M5.6 4.4h4.8' +
      'a1.2 1.2 0 0 1 1.2 1.2v4.8a1.2 1.2 0 0 1-1.2 1.2H5.6a1.2 1.2 0 0 1-1.2-1.2' +
      'V5.6a1.2 1.2 0 0 1 1.2-1.2zM6.4 6.4h3.2v3.2H6.4z' +
      'M6.1 2.2h1.0v2.2h-1.0zM8.9 2.2h1.0v2.2h-1.0z' +
      'M6.1 11.6h1.0v2.2h-1.0zM8.9 11.6h1.0v2.2h-1.0z' +
      'M2.2 6.1h2.2v1.0H2.2zM2.2 8.9h2.2v1.0H2.2z' +
      'M11.6 6.1h2.2v1.0h-2.2zM11.6 8.9h2.2v1.0h-2.2z"/>',

    /* A picture. The frame stays solid and the picture is the hole in it: a sun
     * and a ridge, which is the one composition that survives being 8px wide. */
    'file-image':
      '<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M4.7 3.2h6.6' +
      'a1.3 1.3 0 0 1 1.3 1.3v7.0a1.3 1.3 0 0 1-1.3 1.3H4.7a1.3 1.3 0 0 1-1.3-1.3' +
      'V4.5a1.3 1.3 0 0 1 1.3-1.3zM6.2 4.6a1.0 1.0 0 1 1 0 2.0 1.0 1.0 0 0 1 0-2.0z' +
      'M4.4 11.4 7.3 7.6l1.9 2.4 1.2-1.3 1.7 2.7z"/>',

    /* A symlink. The page it would have been, with the arrow cut through it --
     * what is on the far end is unknown from here, so the icon says "elsewhere"
     * and nothing about what is there. */
    'file-link':
      '<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M4.5 1.8h4.3l3.8 3.8v7.5a1.1 1.1 0 0 1-1.1 1.1H4.5a1.1 1.1 0 0 1-1.1-1.1V2.9a1.1 1.1 0 0 1 1.1-1.1z' +
      'M5.4 8.1h3.0V6.3l2.8 2.5-2.8 2.5V9.5H5.4z"/>',

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
     * It takes `currentColor` like every other glyph here, so it is whatever
     * the thing around it is -- dim on a resting tab, `--ac` on the tab in
     * front. There was a note here saying it was orange, the orange of the
     * application icon, and pointing at an `--ai` token to prove it. No rule
     * ever set that colour on it and the token has since been deleted; the
     * sentence had outlived the intention by long enough to read as fact.
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

    /*
     * A thought balloon: one bubble and two trailing beneath it.
     *
     * It was three equal circles overlapping, on the argument that the holes
     * carry it. They do not -- three circles of the same size meeting at the
     * same depth is a Venn diagram, and at 15px the six internal arcs close up
     * into a grey knot with a scalloped edge. Nothing in it says thinking; the
     * meaning was being carried entirely by which button it sat on.
     *
     * The balloon says it instead, and it says it with the one thing this set
     * is built on: a large empty counter. A brain and a filled cloud were both
     * tried here before and both read as a lump, which is the same failure --
     * all silhouette, no hole. This has a single unbroken outline round a lot
     * of nothing, and the two dots descending to the corner are what make the
     * shape a thought rather than a speech bubble or a full stop.
     *
     * An ellipse and not a rounded rectangle: `terminal` is already a 2.2-radius
     * box, and at this size the two would be the same icon.
     */
    'think':
      '<ellipse cx="8" cy="8" rx="6.1" ry="3.05" transform="rotate(38 8 8)"/>' +
      '<ellipse cx="8" cy="8" rx="6.1" ry="3.05" transform="rotate(-38 8 8)"/>' +
      '<circle cx="8" cy="8" r="1.05" fill="currentColor" stroke="none"/>',

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
   * A square orange tile with the nautilus laid into it in white -- which is the
   * application icon, built out of parts. The icon is a rounded orange square
   * with a white whorl on it, and the mark that stood here before was only the
   * whorl: orange line art on the panel background, with no tile and no white,
   * so the one thing that makes the icon recognisable at 16px was the one thing
   * missing from it.
   *
   * THE CURVE
   *
   * A logarithmic spiral of 1.4 turns, anticlockwise from a mouth at 38 degrees,
   * the radius falling by 8.5 over its length, stroked from 46% of the outer
   * radius at the mouth down to a hairline at the eye on a taper of t^2.1. Those
   * six numbers are the drawing; the bitmap below is what they sample to on a
   * 19x19 grid, fitted to the tile by its own bounding box rather than placed.
   * Nothing here was put where it is by hand, which is why it can be read as a
   * picture in the source and still be the logo.
   *
   * The radius falls by 8.5 and not by 4 because of what the second winding has
   * to be. Shrink too slowly and it comes out six cells tall and one cell wide,
   * which at this resolution is a straight line and a right angle rather than a
   * curl; fast enough, and it closes into a small ring with a single cell of
   * ground at its centre, which is an eye.
   *
   * WHY IT IS SAMPLED IN TWO PASSES
   *
   * A grid cannot draw a stroke thinner than one cell, and the thin end is the
   * entire point of a tapered mark. Sampling by area alone drops the tail below
   * the threshold and breaks it into loose cells -- not a hairline, a dotted
   * line. So every cell the curve passes through is set outright, which floors
   * the stroke at one block, and area is what adds the second, third and fourth
   * block where the stroke really is that wide. The result reads four blocks
   * thick at the mouth and one at the eye, which is the whole of "thick to thin"
   * at a resolution this coarse.
   *
   * WHY BLOCKS AT ALL
   *
   * This is the empty state of the assistant panel, the only place in the window
   * where the mark is decoration rather than identity. Drawn smooth at 100px
   * beside a paragraph of help text it reads as a watermark someone forgot to
   * remove; assembled out of parts, it reads as a thing waiting to be built,
   * which is what an empty conversation is.
   *
   * The blocks stay square with a 0.6-unit gutter at every size. That gutter is
   * the whole effect -- close it and this is just a low-resolution logo.
   */
  var MARK = [
    '...................',
    '...................',
    '........#####......',
    '.....###########...',
    '....#############..',
    '...###############.',
    '..#####.....######.',
    '..####.......####..',
    '.####..........#...',
    '.###...............',
    '.###...###.........',
    '.###..##.##........',
    '.###..#...#........',
    '..###.....#........',
    '..####...##........',
    '...#######.........',
    '.....####..........',
    '...................',
    '...................'
  ];

  /*
   * Every cell is drawn, not only the shell.
   *
   * `--brand` for the ground and white for the whorl, both written as inline
   * styles rather than left to `currentColor`: there are two colours here and
   * only one of them can be inherited. `--brand` is the token the palette panel
   * deliberately does not expose -- it is not a role, it is this logo -- so it
   * is the same orange in both themes, and white on it is the same white the
   * application icon uses.
   */
  window.tshellMark = function (className) {
    var CELL = 4, BLOCK = 3.4, INSET = (CELL - BLOCK) / 2, RADIUS = 0.9;
    var size = MARK.length * CELL;
    var svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('class', className || 'mark');
    svg.setAttribute('viewBox', '0 0 ' + MARK[0].length * CELL + ' ' + size);
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('focusable', 'false');
    for (var y = 0; y < MARK.length; y++) {
      for (var x = 0; x < MARK[y].length; x++) {
        var shell = MARK[y].charAt(x) === '#';
        var block = document.createElementNS(NS, 'rect');
        block.setAttribute('x', x * CELL + INSET);
        block.setAttribute('y', y * CELL + INSET);
        block.setAttribute('width', BLOCK);
        block.setAttribute('height', BLOCK);
        block.setAttribute('rx', RADIUS);
        block.setAttribute('style', 'fill:' + (shell ? '#fff' : 'var(--brand)'));
        svg.appendChild(block);
      }
    }
    return svg;
  };
})();
