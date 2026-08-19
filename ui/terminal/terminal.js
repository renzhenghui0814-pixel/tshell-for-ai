(function () {
  const vscode = acquireVsCodeApi();
  // Nothing on this page is labelled any more, so it asks for no strings.
  const bootstrap = window.tshellBootstrap || {};

  const terminalElement = document.getElementById('terminal');
  /*
   * Switched off means gone. The button that used to say so lives on the window's
   * title bar now and the shell hides it there; what is left here is the
   * shortcut, which has to refuse on its own.
   */

  /*
   * What this terminal paints with: the sixteen ANSI colours, whichever of the
   * four non-ANSI ones the scheme has an opinion about, a font and a size.
   *
   * Resolved by the shell, which is the only frame that knows both the schemes
   * that ship and the ones the user made. Nothing is merged here -- this page
   * is handed a finished answer and applies it.
   */
  let appearance = bootstrap.appearance || {};

  /*
   * What xterm is told to draw with.
   *
   * None of it can come from CSS: xterm draws into a canvas, so the custom
   * properties the rest of the window is built from are invisible to it and this
   * object is the only way in.
   *
   * Two halves, from two places, on purpose. Anything the scheme leaves unset
   * is read back out of the stylesheet, so it follows the window's light/dark
   * theme and the terminal does not disagree with the chrome around it -- which
   * is what the three built-in schemes do, and why a terminal has always looked
   * like part of this window rather than a rectangle pasted into it. A scheme
   * of the user's own may say otherwise, and then it wins.
   */
  function terminalTheme() {
    const styles = getComputedStyle(document.body);
    /*
     * The slider colours are read out of the stylesheet rather than written
     * again here. Custom properties are legible from script, which is also how
     * the fallbacks below reach the theme's own values.
     */
    const scroll = styles.getPropertyValue('--scroll').trim();
    const scrollHover = styles.getPropertyValue('--scroll-hover').trim();
    const colors = appearance.colors || {};
    const background = colors.background || styles.backgroundColor;
    const token = (name) => styles.getPropertyValue(name).trim();
    return {
      background: background,
      foreground: colors.foreground || styles.color,

      /*
       * xterm does not use a native scrollbar. It draws its own, a div in a
       * `.xterm-scrollable-element` inherited from VS Code's, which is why the
       * `::-webkit-scrollbar` rules that cover every other list in the product
       * do nothing here -- there is no scrollbar for them to style. Its colours
       * come through this object and its width through the option below.
       */
      scrollbarSliderBackground: scroll,
      scrollbarSliderHoverBackground: scrollHover,
      scrollbarSliderActiveBackground: scrollHover,

      /*
       * The overview ruler's own border, which it fills as a one-pixel column
       * down the left of its canvas before it looks at whether there is
       * anything to show. Left unset, xterm falls back to the foreground and
       * draws a near-white hairline the height of the terminal.
       *
       * The ruler is hidden outright in terminal.css and this is belt to that
       * brace: the ruler exists only because its width is how xterm's scrollbar
       * width is set, so any colour it paints is a colour nobody asked for.
       */
      overviewRulerBorder: background,
      cursor: colors.cursor || token('--term-cursor'),
      /*
       * What a block cursor paints the glyph underneath it in, so it has to be
       * whatever is behind the cursor rather than whatever the window is: a
       * scheme with its own background would otherwise punch a window-coloured
       * hole through the character the cursor is sitting on.
       */
      cursorAccent: background,
      selectionBackground: colors.selection || token('--term-select'),

      ...ansiOnly(colors)
    };
  }

  /*
   * The sixteen, and only the sixteen.
   *
   * The four above are spelled differently in xterm's theme object --
   * `selectionBackground`, not `selection` -- so spreading the whole set in
   * would leave a `selection` key xterm ignores and, worse, would let an unset
   * slot overwrite the fallback that was just worked out for it.
   */
  function ansiOnly(colors) {
    const out = {};
    for (const [key, value] of Object.entries(colors)) {
      if (value && !NON_ANSI.has(key)) out[key] = value;
    }
    return out;
  }

  const NON_ANSI = new Set(['foreground', 'background', 'cursor', 'selection']);

  /** The scheme's font, or the stack the rest of the window is set in. */
  function fontFamily() {
    return (
      appearance.font ||
      getComputedStyle(document.body).getPropertyValue('--font-mono').trim() ||
      'monospace'
    );
  }

  /** Repaint with whatever `appearance` and the body's classes now say. */
  function repaint() { term.options.theme = terminalTheme(); }

  const term = new Terminal({
    cursorBlink: true,
    convertEol: true,
    fontFamily: fontFamily(),
    fontSize: appearance.fontSize || 13,
    lineHeight: 1.2,
    /*
     * The width of that self-drawn scrollbar, which is otherwise 14px -- the
     * widest thing in the window, against the surface the user looks at most.
     *
     * It is spelled `overviewRuler.width` because in xterm the two share a
     * track: `verticalScrollbarSize` is `overviewRuler?.width || 14`. Setting it
     * also switches on the overview ruler's renderer, which is harmless here --
     * it paints marks for registered decorations and this terminal registers
     * none, so it draws nothing and costs one idle canvas.
     *
     * 10 to match `::-webkit-scrollbar` in shared/components.css; terminal.css
     * then insets the slider by 2px a side, the same way the shared thumb is
     * inset, so the two read as one scrollbar across the whole window.
     */
    overviewRuler: { width: 10 },
    theme: terminalTheme()
  });
  const fitAddon = new FitAddon.FitAddon();
  term.loadAddon(fitAddon);
  term.open(terminalElement);
  term.focus();

  terminalElement.addEventListener('mousedown', () => term.focus());
  // Nothing else is on this page any more, so any click on it means the terminal.
  document.body.addEventListener('mousedown', () => term.focus());
  document.body.addEventListener('contextmenu', (event) => event.preventDefault());
  terminalElement.addEventListener('contextmenu', (event) => {
    event.preventDefault();
    term.focus();
    vscode.postMessage({ type: 'readClipboard' });
  });

  term.onData((data) => vscode.postMessage({ type: 'input', data }));
  term.onSelectionChange(() => {
    if (term.hasSelection()) vscode.postMessage({ type: 'copyText', text: term.getSelection() });
  });

  function resize() {
    /*
     * Never fit against a box that is not on screen.
     *
     * A tab that is not in front is `display: none`, and the fit addon measures
     * the parent's computed width to decide how many columns fit. On a hidden
     * element that measurement is zero, which is not discarded -- the addon
     * clamps to its own floor of two columns and one row and resizes the
     * terminal to it. The buffer is reflowed to two columns, and reflowing it
     * back when the tab returns does not restore the line the cursor is on: the
     * prompt comes back cut off mid-word.
     *
     * The observer fires on the way out and on the way back, so bailing here
     * costs nothing -- the return trip re-fits against a real box.
     */
    if (!terminalElement.clientWidth || !terminalElement.clientHeight) return;
    fitAddon.fit();
    vscode.postMessage({ type: 'resize', cols: term.cols, rows: term.rows });
  }

  function scheduleResize() {
    requestAnimationFrame(() => {
      resize();
      setTimeout(resize, 50);
    });
  }

  window.addEventListener('resize', resize);
  new ResizeObserver(scheduleResize).observe(terminalElement);
  new MutationObserver(repaint).observe(document.body, { attributes: true, attributeFilter: ['class'] });

  /* AI agent ---------------------------------------------------------------- */

  /*
   * A hard-coded Alt+Shift+P used to be caught here, taking the key before
   * xterm could hand it to the shell on the far end. The trick was right and it
   * is now `shared/keys.js` plus one listener in `shared/host.js`: general,
   * editable, and applied to every page rather than to this one alone.
   *
   * Its reasoning lives on there, because it is the reason the whole mechanism
   * works -- `code` and not `key`, since Alt changes the character a key
   * produces on plenty of layouts and on macOS Option+Shift+P is not "P" at all.
   */

  window.addEventListener('message', (event) => {
    const message = event.data;
    if (message.type === 'snapshot') {
      term.reset();
      term.write(message.output || '');
    }
    /*
     * An appearance change, live. Nothing is reconnected and nothing is redrawn
     * by us: assigning `options.theme` makes xterm repaint its whole buffer,
     * scrollback included, on the next frame.
     *
     * The font is the exception, and it is why this is not three lines. A
     * different family or size is a different cell size, so the same box now
     * holds a different number of columns and rows -- and the pty on the other
     * end is still sizing its output to the old pair until it is told. Hence
     * the re-fit, which measures again and sends the new size on.
     */
    if (message.type === 'appearance') {
      const next = message.appearance || {};
      const resized = next.font !== appearance.font || next.fontSize !== appearance.fontSize;
      appearance = next;
      repaint();
      if (resized) {
        term.options.fontFamily = fontFamily();
        term.options.fontSize = appearance.fontSize || 13;
        scheduleResize();
      }
    }
    if (message.type === 'output') term.write(message.data || '');
    if (message.type === 'clipboardText' && message.text) vscode.postMessage({ type: 'input', data: message.text });
    if (message.type === 'connected') {
      if (message.clear) {
        term.reset();
        term.write(message.output || '');
      }
      scheduleResize();
      term.focus();
    }
  });

  scheduleResize();
  vscode.postMessage({ type: 'ready' });
})();
