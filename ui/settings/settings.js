/*
 * Settings.
 *
 * The only page here that VS Code never hosted, because under VS Code these were
 * its own settings UI and its own theme. It follows the same contract as the
 * other four regardless -- bootstrap in, `postMessage` out -- so the shell does
 * not need a special case for it.
 *
 * Its strings are local rather than drawn from vendor/strings.json: that table
 * was exported from the extension's i18n and has no keys for a page the
 * extension did not have. They stay here -- stage 1 gave Rust access to the same
 * exported table rather than a second copy of it, so there is no "real" table
 * elsewhere for these to move into.
 */
(function () {
  'use strict';

  var boot = window.tshellBootstrap || {};
  var vscode = acquireVsCodeApi();
  var language = boot.language === 'en-US' ? 'en-US' : 'zh-CN';
  var theme = boot.theme === 'light' ? 'light' : 'dark';

  var text = {
    'zh-CN': {
      title: '设置',
      appearance: '外观',
      theme: '主题',
      themeHint: '影响窗口、面板和终端配色',
      scheme: '终端外观',
      schemeHint: '十六色、前景背景、字体与字号。改一处即刻生效，已打开的终端连同回滚内容一起重绘',
      schemeName: '方案名',
      schemeFont: '字体',
      schemeSize: '字号',
      schemeNew: '新建',
      schemeCopy: '复制一份',
      schemeReset: '恢复默认',
      schemeDelete: '删除',
      schemeExtra: '前景、背景与光标',
      untitled: '未命名方案',
      copyOf: '{0} 副本',
      newScheme: '我的方案',
      fontDefault: '默认等宽字体',
      followTheme: '跟随主题',
      deleteTitle: '删除方案',
      deleteMessage: '「{0}」会被删掉，二十个颜色一起。这个动作没有撤销。',
      deleteAccept: '删除',
      resetTitle: '恢复默认',
      resetMessage: '「{0}」会变回随产品发布的样子，你对它做的改动全部丢弃。',
      resetAccept: '恢复',
      schemeBroken: '方案文件读不了，现在用的是内置方案。在你修好它之前这里不会写入，以免覆盖掉里面的东西。{0}',
      dark: '深色',
      light: '浅色',
      languageLabel: '语言',
      languageHint: '切换后窗口会重新载入,已打开的标签页会重开',
      zh: '中文',
      en: 'English',
      storage: '存储',
      secrets: '密码存放位置',
      secretsHint: '密码、私钥口令与 API Key',
      keychain: '系统钥匙链',
      file: '本地文件',
      degraded: '未能连上系统钥匙链，密码改存到 {0}，权限 0600（仅当前用户可读）。这比钥匙链弱：任何能以你的身份运行的程序都读得到。Linux 上装一个 Secret Service（如 gnome-keyring）并重启 tshell 即可恢复。原因：{1}',
      configFile: '配置文件',
      unknownReason: '未提供原因'
    },
    'en-US': {
      title: 'Settings',
      appearance: 'Appearance',
      theme: 'Theme',
      themeHint: 'Applies to the window, the panels and the terminal',
      scheme: 'Terminal appearance',
      schemeHint: 'The sixteen colours, the foreground and background, the font and its size. Every edit applies at once, repainting each open terminal including its scrollback',
      schemeName: 'Name',
      schemeFont: 'Font',
      schemeSize: 'Size',
      schemeNew: 'New',
      schemeCopy: 'Duplicate',
      schemeReset: 'Restore',
      schemeDelete: 'Delete',
      schemeExtra: 'Foreground, background and cursor',
      untitled: 'Untitled scheme',
      copyOf: '{0} copy',
      newScheme: 'My scheme',
      fontDefault: 'The default monospace stack',
      followTheme: 'follows the theme',
      deleteTitle: 'Delete scheme',
      deleteMessage: '"{0}" goes, and its twenty colours with it. There is no undoing this.',
      deleteAccept: 'Delete',
      resetTitle: 'Restore scheme',
      resetMessage: '"{0}" goes back to the way it shipped, and every change you made to it is discarded.',
      resetAccept: 'Restore',
      schemeBroken: 'The schemes file could not be read, so the built-in schemes are what you are looking at. Nothing here is saved until you fix it, so that whatever is in there is not written over. {0}',
      dark: 'Dark',
      light: 'Light',
      languageLabel: 'Language',
      languageHint: 'The window reloads, and the tabs you have open are reopened',
      zh: '中文',
      en: 'English',
      storage: 'Storage',
      secrets: 'Secrets are kept in',
      secretsHint: 'Passwords, private key passphrases and API keys',
      keychain: 'the system keychain',
      file: 'a local file',
      degraded: 'The system keychain could not be reached, so secrets are kept in {0} with 0600 permissions (readable only by you). That is weaker than the keychain: anything running as you can read it. On Linux, install a Secret Service provider such as gnome-keyring and restart tshell. Reason: {1}',
      configFile: 'Config file',
      unknownReason: 'no reason given'
    }
  }[language];

  document.title = text.title;
  document.getElementById('page-title').textContent = text.title;
  document.getElementById('appearance-title').textContent = text.appearance;
  document.getElementById('theme-label').textContent = text.theme;
  document.getElementById('theme-hint').textContent = text.themeHint;
  document.getElementById('scheme-label').textContent = text.scheme;
  document.getElementById('scheme-hint').textContent = text.schemeHint;
  document.getElementById('scheme-name-label').textContent = text.schemeName;
  document.getElementById('scheme-font-label').textContent = text.schemeFont;
  document.getElementById('scheme-size-label').textContent = text.schemeSize;
  document.getElementById('scheme-extra-title').textContent = text.schemeExtra;

  /*
   * Where the secrets ended up, stated rather than offered.
   *
   * The keychain is what every supported platform should be using, and the file
   * is what a Linux box with no Secret Service falls back to. Telling the user
   * which one they got is the whole point of the section: a silent downgrade
   * from "the operating system protects this" to "a file permission protects
   * this" is a change in what their passwords are worth, and they cannot weigh
   * it if nobody says it happened.
   */
  var storage = boot.storage || {};
  var onFile = storage.backend === 'file';

  document.getElementById('storage-title').textContent = text.storage;
  document.getElementById('secrets-label').textContent = text.secrets;
  document.getElementById('secrets-hint').textContent = text.secretsHint;
  document.getElementById('config-label').textContent = text.configFile;
  document.getElementById('config-value').textContent = storage.configPath || '—';

  var secretsValue = document.getElementById('secrets-value');
  secretsValue.textContent = onFile ? text.file : text.keychain;
  secretsValue.classList.toggle('degraded', onFile);

  if (onFile) {
    var warning = document.getElementById('secrets-warning');
    warning.textContent = text.degraded
      .replace('{0}', storage.secretsPath || text.file)
      .replace('{1}', storage.reason || text.unknownReason);
    warning.hidden = false;
  }

  var choice = document.getElementById('theme-choice');
  var buttons = {};

  ['dark', 'light'].forEach(function (value) {
    var button = document.createElement('button');
    button.type = 'button';
    button.className = 'c-seg-item';
    button.textContent = text[value];
    button.onclick = function () {
      if (theme === value) return;
      theme = value;
      paint();
      /*
       * The scheme editor is not told here. It reads the four theme colours out
       * of the stylesheet, and the stylesheet does not change until host.js
       * flips the body's class -- which happens when this message comes back
       * round. The observer below is where that arrives, and telling it twice
       * would mean telling it once too early.
       */
      vscode.postMessage({ type: 'setTheme', theme: value });
    };
    buttons[value] = button;
    choice.appendChild(button);
  });

  function paint() {
    Object.keys(buttons).forEach(function (value) {
      var on = value === theme;
      buttons[value].classList.toggle('c-on', on);
      buttons[value].setAttribute('aria-pressed', String(on));
    });
  }

  paint();

  /*
   * The terminal's appearance: a scheme to pick, and everything it stands for.
   *
   * Everything is drawn for the theme the window is *currently* in, because that
   * is the half of a built-in scheme the terminals are painting with right now.
   * Switching the theme above redraws the lot, which is why this is a function
   * rather than markup written once.
   *
   * Nothing here writes to disk directly. An edit in progress -- a colour picker
   * being dragged -- is sent as `schemesApply`, which repaints every open
   * terminal and touches no file; an edit that has settled is sent as
   * `schemesSave`, and what comes back is the normalized file, which replaces
   * the working copy. Redrawing from Rust's answer rather than from what was
   * sent is what stops this page showing a colour the file does not hold.
   */
  (function schemeEditor() {
    var api = window.tshellSchemes;

    var pick = document.getElementById('scheme-pick');
    var nameField = document.getElementById('scheme-name');
    var fontField = document.getElementById('scheme-font');
    var fontOptions = document.getElementById('scheme-fonts');
    var sizeField = document.getElementById('scheme-size');
    var preview = document.getElementById('scheme-preview');
    var ansiBox = document.getElementById('scheme-ansi');
    var extraBox = document.getElementById('scheme-extra');
    var errorBox = document.getElementById('scheme-error');

    var newButton = document.getElementById('scheme-new');
    var copyButton = document.getElementById('scheme-copy');
    var resetButton = document.getElementById('scheme-reset');
    var deleteButton = document.getElementById('scheme-delete');

    newButton.textContent = text.schemeNew;
    copyButton.textContent = text.schemeCopy;
    resetButton.textContent = text.schemeReset;
    deleteButton.textContent = text.schemeDelete;

    /*
     * The file that will not parse.
     *
     * Everything below still works -- the built-in schemes paint a terminal
     * perfectly well -- but nothing is written while this is set. Reading a file
     * badly and then saving over it is how one stray comma costs somebody every
     * scheme they ever made.
     */
    var broken = boot.schemesError || '';
    if (broken) {
      errorBox.textContent = text.schemeBroken.replace('{0}', broken);
      errorBox.classList.add('error');
      errorBox.hidden = false;
    }

    /* The working copy. `api` is pointed at it, so it resolves what we edit. */
    var file;
    var current;
    adopt(boot.schemes);

    function adopt(next) {
      file = next ? JSON.parse(JSON.stringify(next)) : null;
      if (!file || typeof file !== 'object') file = { version: 1, selected: '' };
      if (!Array.isArray(file.schemes)) file.schemes = [];
      if (!file.overrides || typeof file.overrides !== 'object') file.overrides = {};
      api.load(file);
      current = api.resolve(current || file.selected);
    }

    /* -- reading ------------------------------------------------------------ */

    /** The stored record for the current scheme, without creating one. */
    function stored() {
      if (api.isBuiltin(current)) return file.overrides[current] || {};
      return ownScheme(current) || {};
    }

    function ownScheme(id) {
      for (var i = 0; i < file.schemes.length; i += 1) {
        if (file.schemes[i].id === id) return file.schemes[i];
      }
      return null;
    }

    /** The colours this scheme actually stores, as opposed to inherits. */
    function storedColors() {
      var record = stored();
      if (api.isBuiltin(current)) return record[theme === 'light' ? 'light' : 'dark'] || {};
      return record.colors || {};
    }

    /* -- writing ------------------------------------------------------------ */

    /*
     * The record an edit writes into, made if it does not exist yet.
     *
     * Kept apart from `stored()` on purpose: creating a record is what makes a
     * built-in scheme count as edited, and reading one should never do that.
     */
    function record() {
      if (api.isBuiltin(current)) {
        return file.overrides[current] || (file.overrides[current] = {});
      }
      return ownScheme(current);
    }

    /** The colour set an edit writes into, for this scheme and this theme. */
    function colorRecord() {
      var into = record();
      if (!api.isBuiltin(current)) return into.colors || (into.colors = {});
      var half = theme === 'light' ? 'light' : 'dark';
      return into[half] || (into[half] = {});
    }

    var saveTimer = 0;

    /*
     * Repaint every open terminal, and this page's preview with them.
     *
     * Nothing is written, and -- the part that matters -- nothing is rebuilt. A
     * colour picker is a native dialog owned by the input that opened it, so
     * replacing that input while it is being dragged closes it. Only what can
     * change without touching the controls is redrawn here.
     */
    function apply() {
      api.load(file);
      resetButton.disabled = !api.isBuiltin(current) || !api.isEdited(current);
      drawPreview(api.appearance(current, theme));
      vscode.postMessage({ type: 'schemesApply', file: file });
    }

    /** As `apply`, for a change that adds, removes or renames something. */
    function rebuild() {
      draw();
      vscode.postMessage({ type: 'schemesApply', file: file });
    }

    /** Write, unless the file on disk is one we could not read. */
    function save() {
      clearTimeout(saveTimer);
      if (broken) return;
      vscode.postMessage({ type: 'schemesSave', file: file });
    }

    /** For a field being typed into: the terminals follow, the disk waits. */
    function saveSoon() {
      apply();
      clearTimeout(saveTimer);
      saveTimer = setTimeout(save, 400);
    }

    /* -- the four verbs ----------------------------------------------------- */

    function makeId() {
      return 's' + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
    }

    /*
     * A new scheme starts as a copy of what is on screen rather than as twenty
     * blanks. Nobody sets out to choose sixteen colours from nothing; they set
     * out to change two and keep the rest.
     */
    function create(name) {
      var look = api.appearance(current, theme);
      var colors = {};
      api.keys.forEach(function (key) {
        if (look.colors[key]) colors[key] = look.colors[key];
      });
      var scheme = {
        id: makeId(),
        name: name,
        font: stored().font || '',
        fontSize: stored().fontSize || 0,
        colors: colors
      };
      if (!scheme.fontSize) delete scheme.fontSize;
      file.schemes.push(scheme);
      current = scheme.id;
      file.selected = current;
      rebuild();
      save();
      nameField.focus();
      nameField.select();
    }

    newButton.onclick = function () { create(text.newScheme); };
    copyButton.onclick = function () {
      create(text.copyOf.replace('{0}', api.name(current, language) || text.untitled));
    };

    deleteButton.onclick = function () {
      var name = api.name(current, language) || text.untitled;
      askShell({
        title: text.deleteTitle,
        message: text.deleteMessage.replace('{0}', name),
        accept: text.deleteAccept
      }).then(function (ok) {
        if (!ok) return;
        file.schemes = file.schemes.filter(function (scheme) { return scheme.id !== current; });
        current = api.defaultId;
        file.selected = current;
        rebuild();
        save();
      });
    };

    resetButton.onclick = function () {
      var name = api.name(current, language) || text.untitled;
      askShell({
        title: text.resetTitle,
        message: text.resetMessage.replace('{0}', name),
        accept: text.resetAccept
      }).then(function (ok) {
        if (!ok) return;
        // Restoring is dropping the record. There is nothing else to undo:
        // what ships was never overwritten, only layered over.
        delete file.overrides[current];
        rebuild();
        save();
      });
    };

    /*
     * The shell owns the dialogs -- an undecorated window has no `showWarning`
     * to borrow, and the browser's own `confirm` freezes the whole WebView --
     * but the words are this page's, so they travel with the question.
     */
    var pending = new Map();
    var askSeq = 0;

    function askShell(question) {
      return new Promise(function (resolve) {
        var token = 'q' + (askSeq += 1);
        pending.set(token, resolve);
        vscode.postMessage({
          type: 'confirm',
          token: token,
          title: question.title,
          message: question.message,
          accept: question.accept
        });
      });
    }

    /* -- the fields --------------------------------------------------------- */

    pick.onchange = function () {
      current = api.resolve(pick.value);
      file.selected = current;
      rebuild();
      save();
    };

    /*
     * A built-in's name may be cleared, and clearing it puts the shipped one
     * back -- which is why an empty field shows that name as a placeholder
     * rather than as a value.
     *
     * The picker's own entry is relettered here rather than by a rebuild: a
     * rebuild would replace the field being typed into and take the caret with
     * it.
     */
    nameField.oninput = function () {
      record().name = nameField.value;
      var option = pick.options[pick.selectedIndex];
      if (option) option.textContent = nameField.value || nameField.placeholder;
      saveSoon();
    };
    nameField.onchange = save;

    fontField.oninput = function () {
      record().font = fontField.value;
      saveSoon();
    };
    fontField.onchange = save;

    sizeField.oninput = function () {
      var size = parseFloat(sizeField.value);
      if (sizeField.value === '') delete record().fontSize;
      else if (isFinite(size)) record().fontSize = size;
      else return;
      saveSoon();
    };
    sizeField.onchange = save;

    // Asked for once, the first time this page is looked at, because listing
    // them means reading a table out of every font file on the machine.
    vscode.postMessage({ type: 'fontsRequest' });

    /* -- the swatches ------------------------------------------------------- */

    /*
     * `#RRGGBB`, which is the only thing `input[type=color]` will take. The
     * theme's own values arrive as `rgb(...)` from `getComputedStyle`, and the
     * selection colour arrives with an alpha that a chip cannot show -- dropping
     * it is right for a 24px square and wrong for anything else, which is why
     * this is only ever used to fill the picker in.
     */
    function toHex(value) {
      value = String(value || '').trim();
      if (/^#[0-9a-f]{6}$/i.test(value)) return value.toUpperCase();
      if (/^#[0-9a-f]{3}$/i.test(value)) {
        return '#' + value.slice(1).replace(/./g, function (c) { return c + c; }).toUpperCase();
      }
      var parts = value.match(/^rgba?\(([^)]+)\)$/i);
      if (!parts) return '#000000';
      var numbers = parts[1].split(/[,\s/]+/).filter(Boolean).slice(0, 3);
      if (numbers.length < 3) return '#000000';
      return '#' + numbers.map(function (n) {
        var byte = Math.max(0, Math.min(255, Math.round(parseFloat(n))));
        return (byte < 16 ? '0' : '') + byte.toString(16);
      }).join('').toUpperCase();
    }

    /** What the terminal would use for one of the four, absent a scheme's say. */
    function themeValue(key) {
      var styles = getComputedStyle(document.body);
      if (key === 'foreground') return toHex(styles.color);
      if (key === 'background') return toHex(styles.backgroundColor);
      if (key === 'cursor') return toHex(styles.getPropertyValue('--term-cursor'));
      return toHex(styles.getPropertyValue('--term-select'));
    }

    function effective(key, look) {
      return look.colors[key] || themeValue(key);
    }

    function swatch(key, look) {
      var set = storedColors()[key] || '';
      var shown = toHex(effective(key, look));

      var cell = document.createElement('div');
      cell.className = 'swatch' + (set ? '' : ' unset');

      var chip = document.createElement('input');
      chip.type = 'color';
      chip.className = 'swatch-chip';
      chip.value = shown;
      chip.setAttribute('aria-label', key);

      var name = document.createElement('span');
      name.className = 'swatch-name';
      name.textContent = key;

      var hex = document.createElement('input');
      hex.type = 'text';
      hex.className = 'swatch-hex';
      hex.value = set;
      // An unset ANSI slot is inherited and shows what it inherited; an unset
      // one of the four is not inherited at all, it follows the window.
      hex.placeholder = api.extra.indexOf(key) === -1 ? shown : text.followTheme;
      hex.maxLength = 7;
      hex.spellcheck = false;
      hex.setAttribute('aria-label', key);

      // Dragging: the terminals follow every frame, the disk waits for the drop.
      chip.oninput = function () {
        colorRecord()[key] = chip.value.toUpperCase();
        hex.value = chip.value.toUpperCase();
        hex.classList.remove('bad');
        cell.classList.remove('unset');
        apply();
      };
      chip.onchange = save;

      hex.oninput = function () {
        var typed = hex.value.trim();
        if (!typed) {
          // Emptied on purpose: this scheme has no opinion about this slot. The
          // chip then shows what it falls back to, faded, rather than the value
          // that was just given up.
          delete colorRecord()[key];
          hex.classList.remove('bad');
          cell.classList.add('unset');
          apply();
          var inherited = toHex(effective(key, api.appearance(current, theme)));
          chip.value = inherited;
          if (api.extra.indexOf(key) === -1) hex.placeholder = inherited;
          return;
        }
        var parsed = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.test(typed) ? toHex(typed) : '';
        hex.classList.toggle('bad', !parsed);
        if (!parsed) return;
        colorRecord()[key] = parsed;
        chip.value = parsed;
        cell.classList.remove('unset');
        apply();
      };
      /*
       * On the way out, this one field is put back in step with what the scheme
       * actually holds -- so a half-typed value is not left sitting there
       * looking accepted.
       *
       * This field and no other. Redrawing the lot here would replace every
       * swatch in the block, and the thing that takes the focus away is usually
       * a click on the next chip along: that chip would be removed between the
       * press and the release, and the colour picker would never open.
       */
      hex.onblur = function () {
        var held = storedColors()[key] || '';
        hex.value = held;
        hex.classList.remove('bad');
        cell.classList.toggle('unset', !held);
      };
      hex.onchange = save;

      cell.append(chip, name, hex);
      return cell;
    }

    /* -- drawing ------------------------------------------------------------ */

    function draw() {
      api.load(file);
      current = api.resolve(current);
      var look = api.appearance(current, theme);
      var held = stored();
      var builtin = api.isBuiltin(current);

      pick.replaceChildren();
      api.list(language).forEach(function (row) {
        var option = document.createElement('option');
        option.value = row.id;
        option.textContent = row.name || text.untitled;
        pick.appendChild(option);
      });
      pick.value = current;

      nameField.value = held.name || '';
      nameField.placeholder = builtin
        ? api.builtins[current].label[language] || api.builtins[current].label['en-US']
        : text.untitled;
      fontField.value = held.font || '';
      fontField.placeholder = text.fontDefault;
      sizeField.value = held.fontSize || '';
      sizeField.placeholder = String(api.defaultFontSize);

      // Restoring only means something for a built-in that has been edited;
      // deleting only means something for a scheme that is not built in.
      resetButton.disabled = !builtin || !api.isEdited(current);
      deleteButton.disabled = builtin;

      ansiBox.replaceChildren();
      api.ansi.forEach(function (key) { ansiBox.appendChild(swatch(key, look)); });
      extraBox.replaceChildren();
      api.extra.forEach(function (key) { extraBox.appendChild(swatch(key, look)); });

      drawPreview(look);
    }

    /*
     * The scheme as a terminal, which is the only place the four non-ANSI
     * colours are visible as what they actually do. The first line is a default
     * CentOS prompt -- no escape sequence in it, so it is drawn in the
     * foreground, which is what makes "why can I not recolour my prompt"
     * answerable by pointing at a control.
     */
    function drawPreview(look) {
      var fg = effective('foreground', look);
      preview.style.background = effective('background', look);
      preview.style.color = fg;
      preview.style.fontFamily =
        look.font || getComputedStyle(document.body).getPropertyValue('--font-mono').trim();
      preview.style.fontSize = (look.fontSize || api.defaultFontSize) + 'px';

      preview.replaceChildren();
      [
        [['[trade@localhost ~]$ ', fg], ['ls --color', fg]],
        [
          ['Documents', look.colors.brightBlue],
          ['  ', fg],
          ['run.sh', look.colors.brightGreen],
          ['  ', fg],
          ['build.log', fg],
          ['  ', fg],
          ['notes.tar.gz', look.colors.red]
        ],
        [['warning: 3 files changed', look.colors.yellow]],
        [['[trade@localhost ~]$ ', fg]]
      ].forEach(function (line, index) {
        var row = document.createElement('div');
        line.forEach(function (piece) {
          var span = document.createElement('span');
          span.textContent = piece[0];
          if (piece[1]) span.style.color = piece[1];
          row.appendChild(span);
        });
        // A block cursor at the end of the last prompt, drawn rather than typed:
        // no character in a font is reliably a full cell.
        if (index === 3) {
          var caret = document.createElement('span');
          caret.className = 'caret';
          caret.style.background = effective('cursor', look);
          row.appendChild(caret);
        }
        preview.appendChild(row);
      });
    }

    /* -- what comes back ---------------------------------------------------- */

    window.addEventListener('message', function (event) {
      var message = event.data || {};
      if (message.type === 'schemes') {
        if (message.error) {
          broken = message.error;
          errorBox.textContent = text.schemeBroken.replace('{0}', message.error);
          errorBox.classList.add('error');
          errorBox.hidden = false;
          return;
        }
        broken = '';
        errorBox.hidden = true;
        /*
         * Not while a field has the caret in it. Rust's answer is the same
         * value spelled its way, and swapping the field out from under someone
         * mid-word would move their cursor to the end of it.
         */
        var busy = document.activeElement;
        if (busy === nameField || busy === fontField || busy === sizeField) {
          adopt(message.file);
          return;
        }
        adopt(message.file);
        draw();
        return;
      }
      if (message.type === 'fonts') {
        fontOptions.replaceChildren();
        (message.fonts || []).forEach(function (family) {
          var option = document.createElement('option');
          option.value = family;
          fontOptions.appendChild(option);
        });
        return;
      }
      if (message.type === 'confirmed') {
        var resolve = pending.get(message.token);
        if (!resolve) return;
        pending.delete(message.token);
        resolve(!!message.ok);
      }
    });

    draw();
    // The theme control above redraws all of this: what is shown is the half of
    // a built-in scheme in use, and that changes with the theme.
    window.addEventListener('tshell:themechanged', draw);
  }());

  /*
   * The switch can also come from elsewhere -- another settings tab, or a future
   * menu item -- and host.js applies it without telling the page. Watching the
   * body's class keeps this control honest about what is actually on screen.
   */
  new MutationObserver(function () {
    theme = document.body.classList.contains('vscode-light') ? 'light' : 'dark';
    paint();
    // The scheme editor below shows the half of a built-in scheme that is in
    // use, and reads four of its colours out of the stylesheet. Both of those
    // are true only now, once the class is actually on the body.
    window.dispatchEvent(new Event('tshell:themechanged'));
  }).observe(document.body, { attributes: true, attributeFilter: ['class'] });

  /*
   * The language, which is the one setting that cannot take effect in place.
   *
   * Every open tab was handed its labels in its bootstrap and reads them
   * synchronously at load, so there is no message that would re-letter one. The
   * window reloads instead -- which the hint says out loud, because a window
   * that blinks without warning reads as a crash.
   */
  document.getElementById('language-label').textContent = text.languageLabel;
  document.getElementById('language-hint').textContent = text.languageHint;

  var languages = document.getElementById('language-choice');
  [['zh-CN', 'zh'], ['en-US', 'en']].forEach(function (pair) {
    var button = document.createElement('button');
    button.type = 'button';
    button.className = 'c-seg-item' + (language === pair[0] ? ' c-on' : '');
    button.textContent = text[pair[1]];
    button.setAttribute('aria-pressed', String(language === pair[0]));
    button.onclick = function () {
      if (language === pair[0]) return;
      vscode.postMessage({ type: 'setLanguage', language: pair[0] });
    };
    languages.appendChild(button);
  });

  /* The assistant ---------------------------------------------------------- */

  /*
   * The settings the chat panel's own toolbar has no room for.
   *
   * Everything here is a number or a switch, so the whole section is driven from
   * two data attributes rather than a function per row: `data-number` names the
   * field it edits, `data-toggle` names the flag. Adding a setting is then a row
   * of HTML and a pair of strings, which is what keeps this page from growing a
   * paragraph of wiring for every checkbox.
   */
  var aiText = {
    'zh-CN': {
      title: 'AI 助手',
      enabled: '启用助手',
      enabledHint: '关掉后终端和服务器面板都不再显示助手入口',
      generalTitle: '常规',
      memoryTitle: '记忆',
      skillsTitle: '技能',
      logTitle: '日志',
      steps: '单次任务步数上限',
      stepsHint: '0 表示不限，任务做完、被拒或你按停止才结束',
      timeout: '单条命令超时（秒）',
      timeoutHint: '超时会向终端发 Ctrl+C，已经打出来的输出仍然算数',
      output: '单条命令输出保留字符数',
      outputHint: '超出后从中间省略，两头都留着',
      context: '对话上下文预算（字符）',
      contextHint: '超出后把最旧的命令输出折叠掉，命令本身留着。0 表示全带上',
      terminal: '把终端最近输出发给模型',
      terminalHint: '发之前会做脱敏，但这仍然是把屏幕内容交出去',
      lines: '最多发多少行',
      linesHint: '只在上面那项开着时有意义',
      memory: '启用记忆',
      memoryHint: '关掉后不注入已记内容，也不再给模型 remember/forget 两个动作',
      readonly: '额外的只读命令',
      readonlyHint: '空格分隔。你们自己的查询工具写在这里，助手就不必每次都问。只放确实什么都不改的',
      global: '全局记忆上限（字符）',
      globalHint: '对每台机器都成立的少数几条，通常是偏好。0 表示这一档只读',
      server: '本机记忆上限（字符）',
      serverHint: '关于这一台机器的事实。写满会被整条拒绝而不是截断，模型会被告知去腾地方',
      maxchars: '单个技能文件读入上限（字符）',
      maxcharsHint: '超出后从中间省略。够放下一份长流程，又不至于一个文件吃掉整个上下文预算',
      skills: '启用技能',
      skillsHint: '关掉后清单和动作都不进提示词',
      log: '记录与模型的完整往来',
      logHint: '排查"回复解析不了"这类问题时才打开',
      keep: '保留多少份日志',
      keepHint: '开新的时会删掉最旧的',
      on: '开',
      off: '关'
    },
    'en-US': {
      title: 'AI assistant',
      enabled: 'Assistant',
      enabledHint: 'Off removes it from the terminal and the server panel',
      generalTitle: 'General',
      memoryTitle: 'Memory',
      skillsTitle: 'Skills',
      logTitle: 'Log',
      steps: 'Steps one task may take',
      stepsHint: '0 is no limit: it ends when the work is done, refused, or you stop it',
      timeout: 'Command timeout (seconds)',
      timeoutHint: 'A timeout sends Ctrl+C; whatever was printed still counts',
      output: 'Characters of a command’s output kept',
      outputHint: 'Past that the middle is elided and both ends are kept',
      context: 'Conversation budget (characters)',
      contextHint: 'Past that the oldest command output is folded away; the commands stay. 0 carries everything',
      terminal: 'Send recent terminal output',
      terminalHint: 'It is masked first, but this is still handing over what is on your screen',
      lines: 'Lines to send',
      linesHint: 'Only meaningful while the setting above is on',
      memory: 'Use memory',
      memoryHint: 'Off injects nothing and stops offering the model remember and forget',
      readonly: 'Extra read-only commands',
      readonlyHint: 'Space separated. Your own query tools go here so the assistant stops asking. Only ones that genuinely change nothing',
      global: 'Global memory limit (characters)',
      globalHint: 'The few facts true of every machine, usually preferences. 0 makes the scope read-only',
      server: 'Per-server memory limit (characters)',
      serverHint: 'Facts about this one machine. A full scope is refused rather than trimmed, and the model is told to make room',
      maxchars: 'Characters of a skill file read in one step',
      maxcharsHint: 'Past that the middle is elided. Room for a long procedure without one file eating the whole context budget',
      skills: 'Use skills',
      skillsHint: 'Off keeps the manifest and the action out of the prompt',
      log: 'Record the whole exchange with the model',
      logHint: 'Something you switch on to look into a problem, not to leave running',
      keep: 'Log files to keep',
      keepHint: 'The oldest are removed as new ones are opened',
      on: 'On',
      off: 'Off'
    }
  }[language];

  var ai = null;

  function labelAi() {
    var pairs = {
      'ai-title': 'title',
      'ai-enabled-label': 'enabled', 'ai-enabled-hint': 'enabledHint',
      'ai-general-title': 'generalTitle', 'ai-memory-title': 'memoryTitle',
      'ai-skills-title': 'skillsTitle', 'ai-log-title': 'logTitle',
      'ai-steps-label': 'steps', 'ai-steps-hint': 'stepsHint',
      'ai-timeout-label': 'timeout', 'ai-timeout-hint': 'timeoutHint',
      'ai-output-label': 'output', 'ai-output-hint': 'outputHint',
      'ai-context-label': 'context', 'ai-context-hint': 'contextHint',
      'ai-terminal-label': 'terminal', 'ai-terminal-hint': 'terminalHint',
      'ai-lines-label': 'lines', 'ai-lines-hint': 'linesHint',
      'ai-memory-label': 'memory', 'ai-memory-hint': 'memoryHint',
      'ai-readonly-label': 'readonly', 'ai-readonly-hint': 'readonlyHint',
      'ai-global-label': 'global', 'ai-global-hint': 'globalHint',
      'ai-server-label': 'server', 'ai-server-hint': 'serverHint',
      'ai-skills-label': 'skills', 'ai-skills-hint': 'skillsHint',
      'ai-maxchars-label': 'maxchars', 'ai-maxchars-hint': 'maxcharsHint',
      'ai-log-label': 'log', 'ai-log-hint': 'logHint',
      'ai-keep-label': 'keep', 'ai-keep-hint': 'keepHint'
    };
    Object.keys(pairs).forEach(function (id) {
      var node = document.getElementById(id);
      if (node) node.textContent = aiText[pairs[id]];
    });
  }

  /** `agent.maxSteps` on the settings object, read or written. */
  function at(object, path, value) {
    var parts = path.split('.');
    var last = parts.pop();
    var node = object;
    parts.forEach(function (part) { node = node[part] = node[part] || {}; });
    if (value === undefined) return node[last];
    node[last] = value;
    return value;
  }

  /** Only what changed, so a page that knows nine fields cannot drop the tenth. */
  function patch(path, value) {
    var body = {};
    at(body, path, value);
    vscode.postMessage({ type: 'aiWrite', patch: body });
    at(ai, path, value);
  }

  function paintAi() {
    if (!ai) return;
    document.getElementById('ai-section').hidden = false;

    document.querySelectorAll('[data-number]').forEach(function (input) {
      input.value = String(at(ai, input.dataset.number));
    });
    document.querySelectorAll('[data-seconds]').forEach(function (input) {
      input.value = String(Math.round(at(ai, input.dataset.seconds) / 1000));
    });
    document.querySelectorAll('[data-toggle]').forEach(function (group) {
      var on = at(ai, group.dataset.toggle) === true;
      group.replaceChildren();
      [['on', true], ['off', false]].forEach(function (pair) {
        var button = document.createElement('button');
        button.type = 'button';
        button.className = 'c-seg-item' + (on === pair[1] ? ' c-on' : '');
        button.textContent = aiText[pair[0]];
        button.setAttribute('aria-pressed', String(on === pair[1]));
        button.onclick = function () {
          if (on === pair[1]) return;
          patch(group.dataset.toggle, pair[1]);
          paintAi();
        };
        group.appendChild(button);
      });
    });
    /*
     * A list, edited as one line of names.
     *
     * Space separated rather than a row of chips, because what goes in it is
     * two or three command names and a chip editor would be more machinery than
     * the setting is worth. Redrawn from what Rust settled on, so a name that
     * was dropped for not being a name simply is not there afterwards.
     */
    document.getElementById('ai-readonly').value =
      (at(ai, 'agent.readOnlyCommands') || []).join(' ');

    govern();
  }

  /*
   * Grey out what its switch has turned off.
   *
   * `data-needs` names the flag; the block or row carrying it gets `.off` when
   * that flag is not true, and every control inside anything marked `.off` is
   * disabled. `closest` is what makes it nest without a word of extra logic: a
   * memory budget inside a memory group inside the assistant's own switch is
   * unavailable if *any* of the three is off, and it is unavailable because the
   * nearest `.off` ancestor exists, not because this function worked out which
   * one it was.
   *
   * The switch that governs a block is not inside the block it governs, so this
   * can never disable the control that would turn it back on.
   */
  function govern() {
    document.querySelectorAll('#ai-section [data-needs]').forEach(function (node) {
      node.classList.toggle('off', at(ai, node.dataset.needs) !== true);
    });
    document
      .querySelectorAll('#ai-section input, #ai-section button, #ai-section select')
      .forEach(function (control) {
        control.disabled = !!control.closest('.off');
      });
  }

  /*
   * Committed when the field is left, not on every keystroke.
   *
   * Typing "1200" through an input that saves as it goes writes 1, 12, 120 and
   * 1200 -- and the first three are clamped to a minimum on the way through, so
   * the field fights the typist.
   */
  function wireNumbers() {
    document.querySelectorAll('[data-number]').forEach(function (input) {
      input.onchange = function () {
        var value = Number(input.value);
        if (!Number.isFinite(value)) return paintAi();
        patch(input.dataset.number, Math.round(value));
      };
    });
    document.querySelectorAll('[data-seconds]').forEach(function (input) {
      input.onchange = function () {
        var value = Number(input.value);
        if (!Number.isFinite(value)) return paintAi();
        patch(input.dataset.seconds, Math.round(value) * 1000);
      };
    });
    document.getElementById('ai-readonly').onchange = function (event) {
      patch('agent.readOnlyCommands', event.target.value.split(/\s+/).filter(Boolean));
    };
  }

  window.addEventListener('message', function (event) {
    var data = event.data || {};
    if (data.type !== 'aiSettings') return;
    // Redrawn from what Rust settled on, never from what was typed: the clamping
    // happens there, and a page showing the unclamped number would be lying.
    ai = data.settings;
    paintAi();
  });

  labelAi();
  wireNumbers();
  vscode.postMessage({ type: 'aiRead' });
})();
