/*
 * The host bridge, standing where VS Code's webview API used to.
 *
 * Each page under ui/ still opens with `acquireVsCodeApi()` and still reads
 * `window.tshellBootstrap`. Neither of those is VS Code any more -- this file
 * supplies both -- and that is deliberate: keeping the page-side contract
 * unchanged is what let 7600 lines of front end move hosts without being edited.
 *
 * Pages do not talk to Tauri. They talk to the shell frame, which owns the only
 * IPC channel in the window. One owner means one place to audit what crosses the
 * boundary, and it keeps the pages loadable in a plain browser for debugging.
 *
 * Bootstrap arrives in the URL fragment rather than over a message, because the
 * pages read it synchronously at the top of their own scripts. A fragment is
 * readable before the first line of page code runs; a message is not.
 */
(function () {
  'use strict';

  var frame = { paneId: 'unknown', bootstrap: { strings: {} } };
  try {
    if (location.hash.length > 1) {
      frame = JSON.parse(decodeURIComponent(location.hash.slice(1)));
    }
  } catch (error) {
    console.error('tshell: unreadable bootstrap fragment', error);
  }

  /*
   * The four pages each named their bootstrap global differently under VS Code
   * -- `tshellBootstrap`, `__tshell`, `__tshellChat`, `__tshellServers`. Setting
   * all four costs three assignments and saves editing four files, and a page
   * only ever reads its own.
   */
  var bootstrap = frame.bootstrap || { strings: {} };
  window.tshellBootstrap = bootstrap;
  window.__tshell = bootstrap;
  window.__tshellChat = bootstrap;
  window.__tshellServers = bootstrap;
  window.tshellPaneId = frame.paneId;

  /*
   * Theme travels two ways at once, because the pages read it two ways.
   *
   * `data-theme` on the root drives shared/theme.css, which is where the
   * `--vscode-*` variables come from. The `vscode-light` / `vscode-dark` class
   * on the body is VS Code's own convention, and the pages still watch for it:
   * highlight.css keys its token colours off it, and terminal.js has a
   * MutationObserver on the body's class list that re-themes xterm the moment it
   * changes. Setting both is what makes a live switch reach the canvas that
   * xterm draws into -- CSS variables alone never could.
   */
  function applyTheme(next) {
    var light = next === 'light';
    if (light) document.documentElement.setAttribute('data-theme', 'light');
    else document.documentElement.removeAttribute('data-theme');

    var mark = function () {
      document.body.classList.toggle('vscode-light', light);
      document.body.classList.toggle('vscode-dark', !light);
    };
    // This file runs in <head>, so on first call there is no body yet.
    if (document.body) mark();
    else document.addEventListener('DOMContentLoaded', mark);
  }

  applyTheme(frame.theme);

  /*
   * The user's edits to the palette, applied before anything is drawn.
   *
   * They ride the fragment for the same reason the bootstrap does: a page reads
   * its fragment synchronously, and a page that had to wait for a message would
   * paint once in the shipped palette and once in the user's. Both halves are
   * written at once, so the theme switch above needs nothing from here.
   *
   * `palette.js` has to be loaded before this file for a page to wear them. A
   * page that does not load it is not broken -- it wears the palette that
   * ships, which is a complete one.
   */
  function applyPalette(file) {
    if (!window.tshellPalette) return;
    try {
      window.tshellPalette.apply(document, file);
    } catch (error) {
      // A palette is decoration. Nothing here is worth failing a page load over.
      console.error('tshell: could not apply the palette', error);
    }
  }

  applyPalette(frame.palette);

  var stateKey = 'tshell:state:' + frame.paneId;

  var api = {
    postMessage: function (message) {
      parent.postMessage({ __tshell: 'page', paneId: frame.paneId, payload: message }, '*');
    },
    getState: function () {
      try {
        var raw = sessionStorage.getItem(stateKey);
        return raw ? JSON.parse(raw) : undefined;
      } catch (error) {
        return undefined;
      }
    },
    setState: function (state) {
      try {
        sessionStorage.setItem(stateKey, JSON.stringify(state));
      } catch (error) {
        // A full or disabled store is not worth failing a keystroke over.
      }
      return state;
    }
  };

  window.acquireVsCodeApi = function () {
    return api;
  };

  /*
   * Tell the shell when this pane is being used.
   *
   * With the window split into columns, "which pane is in front" is no longer
   * the same question as "which tab was last clicked": several panes are on
   * screen at once, and clicking into one of them is an event that never leaves
   * this frame. The shell needs it -- the title bar's actions follow the focused
   * column, and that is where the next tab opens.
   *
   * Capturing, so a page that stops the event on its own way down still reports.
   * This is the one message the bridge sends of its own accord; everything else
   * here only carries what the page asked to send.
   */
  function reportFocus() {
    api.postMessage({ type: 'paneFocus' });
  }

  window.addEventListener('pointerdown', reportFocus, true);
  window.addEventListener('focus', reportFocus);

  /*
   * Unwrap what the shell sends and re-raise it as the bare `message` event the
   * pages already listen for. The synthetic event carries no `__tshell` key, so
   * the listener below ignores it and there is no loop.
   */
  window.addEventListener('message', function (event) {
    var data = event.data;
    if (!data || data.__tshell !== 'host') return;

    // Theme is the host's business, not the page's. Handled here and not passed
    // on, so no page needs a case for a message it never had under VS Code.
    if (data.payload && data.payload.type === 'theme') {
      applyTheme(data.payload.theme);
      return;
    }

    // Same reasoning: which colours the window is made of is the host's
    // business, and no page ever had a case for it.
    if (data.payload && data.payload.type === 'palette') {
      applyPalette(data.payload.palette);
      return;
    }

    window.dispatchEvent(new MessageEvent('message', { data: data.payload }));
  });
})();
