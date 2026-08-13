/*
 * What a terminal looks like, as named schemes.
 *
 * A scheme is the sixteen ANSI colours, the four that are not ANSI (foreground,
 * background, cursor, selection), a font and a size. Three ship with the
 * product and are written out below; the rest are the user's, and they live in
 * `tshell.schemes.json` under `schemes.rs`.
 *
 * The built-in three cannot live in theme.css. xterm draws into a canvas, so
 * CSS custom properties are invisible to it and the only way in is the `theme`
 * object it is constructed with -- which is why this is a script and not a
 * stylesheet, and why the merge below happens here rather than in Rust.
 *
 * Loaded by the shell (which owns the merge and hands the result out) and by
 * settings (which shows and edits it). Not by the terminal page: that one is
 * given a finished appearance and has nothing to resolve.
 */
(function () {
  'use strict';

  /* The sixteen, in the order a picker lists them: the plain half, then bright. */
  var ANSI = [
    'black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
    'brightBlack', 'brightRed', 'brightGreen', 'brightYellow',
    'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite'
  ];

  /*
   * The four that are not ANSI colours.
   *
   * Absent on a built-in scheme, and absent means "follow the window's own
   * light/dark theme" -- which is what kept the terminal from ever disagreeing
   * with the chrome around it, and what the terminal page reads out of its
   * stylesheet when it finds nothing here. A scheme of your own may fill them
   * in, and filling `foreground` in is the only way to recolour a prompt whose
   * PS1 carries no escape sequence of its own.
   */
  var EXTRA = ['foreground', 'background', 'cursor', 'selection'];

  var KEYS = ANSI.concat(EXTRA);

  /*
   * A scheme that names no font comes back naming none, rather than naming the
   * default. The default is `--font-mono` in theme.css, and a custom property
   * is legible from script -- so the page that needs a font reads it from the
   * stylesheet instead of this file holding a second copy of the stack that
   * could drift from the first.
   */
  var DEFAULT_FONT_SIZE = 13;

  var BUILTINS = {
    /*
     * The default, designed alongside the rest of the product: green is the
     * `--ok` of the connected dot, red is `--err`, yellow is `--warn`, blue
     * leans towards the accent. A red `error` in a build log and a red "not
     * connected" in the status bar are the same red.
     *
     * Every colour clears 4.5:1 against its own background, `black` excepted --
     * ANSI black is what a program picks when it means "the background", and
     * lifting it to be legible against that background defeats its only job.
     * On light the bright half is darker and more saturated rather than
     * lighter, because "brighter" on a white ground means less contrast.
     */
    nebula: {
      label: { 'zh-CN': '星轨 Nebula', 'en-US': 'Nebula' },
      dark: {
        black: '#2A3040', red: '#FF6B6B', green: '#35D6A4', yellow: '#F5B546',
        blue: '#6E9BF7', magenta: '#C48CF7', cyan: '#4FD3E0', white: '#C3CAD9',
        brightBlack: '#6E7C98', brightRed: '#FF8F8F', brightGreen: '#5FE8BC', brightYellow: '#FFD07A',
        brightBlue: '#8FB4FF', brightMagenta: '#D6A8FF', brightCyan: '#78E6F0', brightWhite: '#EDF0F7'
      },
      light: {
        black: '#2A3040', red: '#C7443C', green: '#0E8563', yellow: '#9A6B05',
        blue: '#3A54D8', magenta: '#8B36C4', cyan: '#0C7C8C', white: '#6B7280',
        brightBlack: '#4A5468', brightRed: '#9E342D', brightGreen: '#0B654B', brightYellow: '#755104',
        brightBlue: '#2B47D5', brightMagenta: '#7E31B2', brightCyan: '#09616D', brightWhite: '#1A1E2B'
      }
    },

    /*
     * What the terminal looked like before this window had a palette of its
     * own: VS Code's Dark+ and Light+ ANSI sets, to the digit. Here for the
     * people who spent years reading these exact colours and would rather not
     * relearn what a yellow means.
     */
    vscode: {
      label: { 'zh-CN': 'VS Code 经典', 'en-US': 'VS Code Classic' },
      dark: {
        black: '#000000', red: '#CD3131', green: '#0DBC79', yellow: '#E5E510',
        blue: '#2472C8', magenta: '#BC3FBC', cyan: '#11A8CD', white: '#E5E5E5',
        brightBlack: '#666666', brightRed: '#F14C4C', brightGreen: '#23D18B', brightYellow: '#F5F543',
        brightBlue: '#3B8EEA', brightMagenta: '#D670D6', brightCyan: '#29B8DB', brightWhite: '#FFFFFF'
      },
      light: {
        black: '#000000', red: '#A31515', green: '#008000', yellow: '#795E26',
        blue: '#0451A5', magenta: '#AF00DB', cyan: '#008080', white: '#555555',
        brightBlack: '#666666', brightRed: '#CD3131', brightGreen: '#14CE14', brightYellow: '#B5A642',
        brightBlue: '#0000FF', brightMagenta: '#AF00DB', brightCyan: '#008080', brightWhite: '#000000'
      }
    },

    /*
     * Ethan Schoonover's Solarized, in its published ANSI mapping. One set for
     * both themes, which is the whole idea of it: the sixteen stay put and only
     * the background and foreground swap, so a colour means the same thing in
     * either. Left exactly as published -- adjusting Solarized for contrast
     * would leave something that is no longer Solarized.
     */
    solarized: {
      label: { 'zh-CN': 'Solarized', 'en-US': 'Solarized' },
      dark: {
        black: '#073642', red: '#DC322F', green: '#859900', yellow: '#B58900',
        blue: '#268BD2', magenta: '#D33682', cyan: '#2AA198', white: '#EEE8D5',
        brightBlack: '#002B36', brightRed: '#CB4B16', brightGreen: '#586E75', brightYellow: '#657B83',
        brightBlue: '#839496', brightMagenta: '#6C71C4', brightCyan: '#93A1A1', brightWhite: '#FDF6E3'
      },
      light: {
        black: '#073642', red: '#DC322F', green: '#859900', yellow: '#B58900',
        blue: '#268BD2', magenta: '#D33682', cyan: '#2AA198', white: '#EEE8D5',
        brightBlack: '#002B36', brightRed: '#CB4B16', brightGreen: '#586E75', brightYellow: '#657B83',
        brightBlue: '#839496', brightMagenta: '#6C71C4', brightCyan: '#93A1A1', brightWhite: '#FDF6E3'
      }
    }
  };

  /* The order the built-ins are listed in, which an object's keys do not promise. */
  var BUILTIN_ORDER = ['nebula', 'vscode', 'solarized'];
  var DEFAULT_ID = 'nebula';

  /*
   * The file, as Rust last handed it over. Replaced wholesale rather than
   * mutated: `schemes_save` returns the normalized file, and taking that as the
   * truth is what keeps the page from showing a colour the file does not say.
   */
  var file = emptyFile();

  function emptyFile() {
    return { version: 1, selected: '', schemes: [], overrides: {} };
  }

  function userScheme(id) {
    for (var i = 0; i < file.schemes.length; i += 1) {
      if (file.schemes[i].id === id) return file.schemes[i];
    }
    return null;
  }

  function isBuiltin(id) {
    return Object.prototype.hasOwnProperty.call(BUILTINS, id);
  }

  /** Copy the colour slots that are actually set. Absent never overwrites. */
  function layer(onto, from) {
    if (!from) return onto;
    KEYS.forEach(function (key) {
      if (from[key]) onto[key] = from[key];
    });
    return onto;
  }

  window.tshellSchemes = {
    ansi: ANSI,
    extra: EXTRA,
    keys: KEYS,
    builtins: BUILTINS,
    builtinOrder: BUILTIN_ORDER,
    defaultId: DEFAULT_ID,
    defaultFontSize: DEFAULT_FONT_SIZE,

    /** Take the file Rust handed over. Anything missing reads as nothing set. */
    load: function (loaded) {
      file = loaded && typeof loaded === 'object' ? loaded : emptyFile();
      if (!Array.isArray(file.schemes)) file.schemes = [];
      if (!file.overrides || typeof file.overrides !== 'object') file.overrides = {};
      return file;
    },

    /** The file as it stands, for the settings page to edit a copy of. */
    file: function () { return file; },

    isBuiltin: isBuiltin,

    /** A real id: the one asked for if we have it, the default if we do not. */
    resolve: function (id) {
      return isBuiltin(id) || userScheme(id) ? id : DEFAULT_ID;
    },

    /** Whichever scheme the terminals should be painting with. */
    selected: function () { return this.resolve(file.selected); },

    /**
     * What this scheme is called, in this language.
     *
     * A built-in has a name in both languages and a rename replaces both, which
     * is right: a name the user typed is not a phrase to be translated.
     */
    name: function (id, language) {
      var edit = file.overrides[id];
      if (edit && edit.name) return edit.name;
      if (isBuiltin(id)) {
        var label = BUILTINS[id].label;
        return label[language] || label['en-US'];
      }
      var own = userScheme(id);
      return own ? own.name : '';
    },

    /** Every scheme, built-ins first, in the order the picker lists them. */
    list: function (language) {
      var self = this;
      var rows = BUILTIN_ORDER.map(function (id) {
        return { id: id, name: self.name(id, language), builtin: true };
      });
      file.schemes.forEach(function (scheme) {
        rows.push({ id: scheme.id, name: scheme.name, builtin: false });
      });
      return rows;
    },

    /*
     * True if this built-in has been edited, which is what "restore" undoes.
     *
     * A record that says nothing does not count. One gets made the moment a
     * field is touched -- clearing a font size makes one just as surely as
     * setting one does -- and a "restore" button that lights up because you
     * emptied a box and put it back is offering to undo nothing. Rust drops
     * these on the next save; this is the same judgement, made a round trip
     * earlier so the button is right immediately.
     */
    isEdited: function (id) {
      var edit = isBuiltin(id) && file.overrides[id];
      if (!edit) return false;
      if (edit.name || edit.font || edit.fontSize) return true;
      return ['colors', 'dark', 'light'].some(function (half) {
        return edit[half] && KEYS.some(function (key) { return edit[half][key]; });
      });
    },

    /**
     * The finished appearance: what xterm is handed, and what the settings page
     * draws.
     *
     * Three layers for a built-in -- what ships, then the user's edit to this
     * half of it -- and one for a scheme of the user's own, which has a single
     * set because a scheme that brings its own background is already either a
     * dark one or a light one.
     *
     * The four non-ANSI slots may come back absent. That is not a gap to be
     * filled in here: the terminal page reads them off its own stylesheet, so
     * absent means the scheme is content to follow the window.
     */
    appearance: function (id, theme) {
      id = this.resolve(id);
      var half = theme === 'light' ? 'light' : 'dark';
      var colors = {};
      var font = '';
      var fontSize = 0;

      if (isBuiltin(id)) {
        layer(colors, BUILTINS[id][half]);
        var edit = file.overrides[id];
        if (edit) {
          layer(colors, edit[half]);
          font = edit.font || '';
          fontSize = edit.fontSize || 0;
        }
      } else {
        var own = userScheme(id);
        /*
         * The default scheme underneath, so that a slot the user never filled
         * in is a legible colour rather than a hole. Emptying a swatch is not
         * "make this one invisible" -- xterm falls back to a palette of its own
         * that has nothing to do with this window -- it is "I have no opinion
         * about this one", and this is what having no opinion should look like.
         */
        layer(colors, BUILTINS[DEFAULT_ID][half]);
        layer(colors, own.colors);
        font = own.font || '';
        fontSize = own.fontSize || 0;
      }

      // `font` may come back empty, which means the stylesheet's `--font-mono`.
      // The size never does: xterm needs a number and 13 is what it was.
      return {
        id: id,
        colors: colors,
        font: font,
        fontSize: fontSize || DEFAULT_FONT_SIZE
      };
    }
  };
})();
