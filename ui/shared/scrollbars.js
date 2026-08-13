/*
 * Which box the pointer is in, so its scrollbars can be the only ones drawn.
 *
 * The bars themselves are styled in components.css and are transparent until
 * something marks their container `c-scroll-hot`. This file is what marks it.
 *
 * It should not need to exist. `:hover::-webkit-scrollbar-thumb` says the same
 * thing in one line of CSS and it half works: Chromium repaints a custom
 * scrollbar when the pointer crosses onto the scroller's *own* box -- which in
 * practice means the scrollbar gutter and the padding around the content -- and
 * not when it crosses onto a child. A file list is wall-to-wall rows, so the
 * pointer is always on a child, and the bars only appeared once you had already
 * found the strip they live in. A class lands on the element itself and forces
 * a style recalculation there, and that does invalidate the scrollbar.
 *
 * Every scrollable ancestor is marked, not just the nearest, because the two
 * axes of one visible box are not always one element: in the transfer page the
 * rows scroll up and down inside `.list` while the columns scroll sideways in
 * the `.pane` around it. Entering the list should show both.
 */
(function () {
  'use strict';

  var HOT = 'c-scroll-hot';
  var marked = [];
  var queued = false;
  var target = null;

  /*
   * Overflowing *and* allowed to scroll about it. The second half matters:
   * plenty of boxes here are `overflow: hidden` around content larger than
   * themselves, and none of those has a bar to show.
   */
  function scrolls(node) {
    var style = getComputedStyle(node);
    if (node.scrollHeight > node.clientHeight && /auto|scroll/.test(style.overflowY)) return true;
    return node.scrollWidth > node.clientWidth && /auto|scroll/.test(style.overflowX);
  }

  function chainFrom(node) {
    var out = [];
    for (; node && node.nodeType === 1; node = node.parentElement) {
      if (scrolls(node)) out.push(node);
    }
    return out;
  }

  function mark(next) {
    marked.forEach(function (node) {
      if (next.indexOf(node) === -1) node.classList.remove(HOT);
    });
    next.forEach(function (node) { node.classList.add(HOT); });
    marked = next;
  }

  /*
   * Coalesced to one measurement per frame. `scrolls` reads `scrollHeight`,
   * which forces layout, and `pointerover` fires on every element the pointer
   * crosses -- once per row while running down a file list.
   */
  function schedule() {
    if (queued) return;
    queued = true;
    requestAnimationFrame(function () {
      queued = false;
      mark(target ? chainFrom(target) : []);
    });
  }

  document.addEventListener('pointerover', function (event) {
    target = event.target;
    schedule();
  }, true);

  /*
   * A `pointerout` with nothing to go to is the pointer leaving the document.
   * Leaving for another element is the ordinary case and `pointerover` has
   * already handled it.
   */
  document.addEventListener('pointerout', function (event) {
    if (event.relatedTarget) return;
    target = null;
    schedule();
  }, true);

  // The pointer can also leave by the window losing focus, and no pointer event
  // is sent for that. Without this the bars stay up in a pane nobody is in.
  window.addEventListener('blur', function () {
    target = null;
    schedule();
  });
})();
