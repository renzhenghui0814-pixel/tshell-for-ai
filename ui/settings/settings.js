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
      palette: '窗口配色',
      paletteHint: '深浅两套各十八个颜色，其余的（悬停、描边、填充上的文字、焦点环）由它们算出来。没改过的颜色不写进文件，跟随产品更新',
      paletteSurface: '底色与填充',
      paletteInk: '文字',
      paletteMeaning: '语义色',
      paletteReset: '恢复默认',
      paletteResetTitle: '恢复默认配色',
      paletteResetMessage: '{0}这一半你改过的颜色全部丢弃，变回随产品发布的样子。这个动作没有撤销。',
      paletteResetAccept: '恢复',
      paletteBlind: '正在编辑{0}，窗口当前是{1}。改动照样保存，但只有下面这块预览看得见。',
      paletteBroken: '配色文件读不了，现在用的是内置配色。在你修好它之前这里不会写入，以免覆盖掉里面的东西。{0}',
      paletteFont: '界面字体',
      paletteMono: '等宽字体',
      paletteFontDefault: '默认界面字体',
      paletteMonoDefault: '默认等宽字体',
      paletteContrast: '这些搭配低于 4.5:1，正常视力在普通屏幕上也许还看得清，其他人不一定：',
      paletteOn: '{0} 压在 {1} 上',
      paletteSample: '示例文字 Aa 0123',
      paletteSampleDim: '次一级的说明文字',
      paletteBgInput: '输入框',
      paletteBgCode: '代码块与命令输出',
      paletteBgBase: '面板底色（也是终端的）',
      paletteBgElev: '窗口底色（标题栏与缝隙）',
      paletteBgTab: '当前标签',
      paletteBgCard: '卡片',
      paletteBgDialog: '对话框',
      paletteBgMenu: '菜单',
      paletteAc: '强调色',
      paletteChatUser: '用户消息',
      paletteProgress: '传输进度条',
      paletteTx: '正文',
      paletteTxDim: '次级文字',
      paletteTxFaint: '极淡（只画线，不承载文字）',
      paletteOk: '成功',
      paletteWarn: '警告',
      paletteErr: '错误',
      paletteInfo: '链接与提示',
      paletteAcTx: '强调色上的文字',
      paletteErrTx: '错误色上的文字',
      paletteChatUserTx: '用户消息上的文字',
      keysTitle: '快捷键',
      keysHint: '在终端里按下也有效——组合键会被窗口拦下，不会发给远端的 shell',
      keysReset: '恢复默认',
      keysCapturing: '按下组合键，Esc 取消',
      keysChange: '点击后按下新的组合键',
      keysOpenTransfer: '打开文件传输',
      keysOpenAssistant: '打开 AI 助手',
      keysToggleFullscreen: '全屏 / 退出全屏',
      keysNeedsShift: '{0} 不能用：除功能键外，快捷键必须带 Shift。不带 Shift 的组合是远端 shell 的——Ctrl+A 是行首、Ctrl+E 是行尾、Ctrl+R 是搜索历史，被窗口拿走就再也传不过去了',
      keysNeedsModifier: '{0} 不能用：还需要 Ctrl、Alt 或 Meta 之一',
      keysModifierOnly: '这只是一个修饰键，还需要一个主键',
      keysBadShape: '这个按键认不出来，换一个',
      keysTaken: '{0} 已经绑给「{1}」了',
      pickerField: '饱和度与明度，方向键微调',
      pickerHue: '色相',
      pickerHex: '十六进制颜色值',
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
      palette: 'Window palette',
      paletteHint: 'Eighteen colours per half. The rest -- hovers, hairlines, the ink on a fill, the focus ring -- follow from them. A colour you never change is not stored, so it keeps up with the product',
      paletteSurface: 'Surfaces and fills',
      paletteInk: 'Text',
      paletteMeaning: 'Meanings',
      paletteReset: 'Restore defaults',
      paletteResetTitle: 'Restore the palette',
      paletteResetMessage: 'Every colour you changed in the {0} half is discarded and goes back to the way it shipped. There is no undoing this.',
      paletteResetAccept: 'Restore',
      paletteBlind: 'Editing the {0} half while the window is {1}. Changes still save; the preview below is the only place they show.',
      paletteBroken: 'The palette file could not be read, so the palette that ships is what you are looking at. Nothing here is saved until you fix it, so that whatever is in there is not written over. {0}',
      paletteFont: 'Interface font',
      paletteMono: 'Monospace font',
      paletteFontDefault: 'The default interface stack',
      paletteMonoDefault: 'The default monospace stack',
      paletteContrast: 'These pairs fall below 4.5:1. They may read fine to you on a good monitor and not at all to everyone else:',
      paletteOn: '{0} on {1}',
      paletteSample: 'Sample text Aa 0123',
      paletteSampleDim: 'the line under it',
      paletteBgInput: 'Input fields',
      paletteBgCode: 'Code blocks and command output',
      paletteBgBase: 'Panel surface (and the terminal)',
      paletteBgElev: 'Window ground (title bar and the gaps)',
      paletteBgTab: 'The tab in front',
      paletteBgCard: 'Cards',
      paletteBgDialog: 'Dialogs',
      paletteBgMenu: 'Menus',
      paletteAc: 'Accent',
      paletteChatUser: 'User message',
      paletteProgress: 'Transfer progress bar',
      paletteTx: 'Text',
      paletteTxDim: 'Secondary text',
      paletteTxFaint: 'Faint (hairlines only, never words)',
      paletteOk: 'Success',
      paletteWarn: 'Warning',
      paletteErr: 'Error',
      paletteInfo: 'Links and hints',
      paletteAcTx: 'the ink on the accent',
      paletteErrTx: 'the ink on the error colour',
      paletteChatUserTx: 'the ink on a user message',
      keysTitle: 'Shortcuts',
      keysHint: 'These work inside a terminal too -- the window takes the combination before it can be sent to the shell on the far end',
      keysReset: 'Restore defaults',
      keysCapturing: 'Press the combination; Esc to cancel',
      keysChange: 'Click, then press the new combination',
      keysOpenTransfer: 'Open file transfer',
      keysOpenAssistant: 'Open the AI assistant',
      keysToggleFullscreen: 'Full screen on and off',
      keysNeedsShift: '{0} cannot be used: apart from the function keys, a shortcut has to include Shift. What it would otherwise take belongs to the shell on the far end -- Ctrl+A is the start of the line, Ctrl+E the end, Ctrl+R the history search -- and a key this window keeps is a key that never gets there',
      keysNeedsModifier: '{0} cannot be used: it also needs Ctrl, Alt or Meta',
      keysModifierOnly: 'That is only a modifier; it needs a key as well',
      keysBadShape: 'That key was not recognised. Try another',
      keysTaken: '{0} is already bound to "{1}"',
      pickerField: 'Saturation and value; arrow keys nudge',
      pickerHue: 'Hue',
      pickerHex: 'Hex colour value',
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

  /*
   * What the body's class list says right now, as opposed to what the control
   * says. They differ for exactly as long as it takes a click here to reach the
   * shell and come back. See the observer at the bottom of this file.
   */
  var wearing = theme;

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
   * -- colours, and the picker for them ------------------------------------
   *
   * Shared by the two editors below: the window's palette and the terminal's
   * schemes. One picker because there is one question -- "which colour is this"
   * -- and two answers to it drawn differently would be two controls to learn.
   *
   * `#RRGGBB`, which is the only shape the swatches deal in. Values read back
   * out of the stylesheet arrive as `rgb(...)`, and a translucent one arrives
   * with an alpha that a chip cannot show -- dropping it is right for a 24px
   * square and wrong for anything else, which is why this is only ever used to
   * fill a chip or a picker in.
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

  /*
   * -- the colour picker we draw ourselves ---------------------------------
   *
   * `<input type="color">` is not usable here, and waiting for it to become so
   * is waiting on someone else's bug.
   *
   * Its picker is a host dialog rather than anything Blink draws, and in a
   * frameless Tauri window WebView2 opens it at a window position it captured
   * earlier -- so once the window has been moved the dialog appears somewhere
   * off the screen and clicking a colour does nothing whatsoever. That is
   * tauri#3089, closed as invalid because it is WebView2's behaviour and not
   * Tauri's to fix.
   *
   * So the chip is a button and the picker is ours. The shell reached the same
   * conclusion about `ask` and `confirm` for the same reason, and this is that
   * rule applied to the one control that had been left out of it.
   *
   * It is the shape everyone already knows: a field you point at to say which
   * colour, a bar under it to say which hue, and the hex if you have one to
   * paste. The first version of this was three labelled sliders -- H, S and L
   * -- which is the same information and none of the affordance: picking a
   * colour is an act of aim, and nobody aims by typing coordinates. Sliders are
   * still what the hue bar is, because a hue *is* one dimension, and the
   * keyboard still reaches everything.
   *
   * HSV rather than HSL for the field, which is what every other picker uses
   * and why they all look like this: at full value and full saturation the
   * corner is the pure hue, and dragging left or down from it never leaves the
   * colours a person is looking for. The HSL square has its pure hues stranded
   * on a line through the middle.
   */
  function hsv(value) {
    var hex = toHex(value);
    var r = parseInt(hex.slice(1, 3), 16) / 255;
    var g = parseInt(hex.slice(3, 5), 16) / 255;
    var b = parseInt(hex.slice(5, 7), 16) / 255;
    var max = Math.max(r, g, b);
    var span = max - Math.min(r, g, b);
    var h = 0;
    if (span) {
      if (max === r) h = ((g - b) / span) % 6;
      else if (max === g) h = (b - r) / span + 2;
      else h = (r - g) / span + 4;
      h *= 60;
      if (h < 0) h += 360;
    }
    return [h, max ? span / max : 0, max];
  }

  function fromHsv(parts) {
    var h = ((parts[0] % 360) + 360) % 360;
    var s = Math.max(0, Math.min(1, parts[1]));
    var v = Math.max(0, Math.min(1, parts[2]));
    var c = v * s;
    var x = c * (1 - Math.abs(((h / 60) % 2) - 1));
    var m = v - c;
    var rgb = h < 60 ? [c, x, 0]
      : h < 120 ? [x, c, 0]
      : h < 180 ? [0, c, x]
      : h < 240 ? [0, x, c]
      : h < 300 ? [x, 0, c]
      : [c, 0, x];
    return '#' + rgb.map(function (part) {
      var byte = Math.round((part + m) * 255);
      return (byte < 16 ? '0' : '') + byte.toString(16);
    }).join('').toUpperCase();
  }

  /* The one open picker, because two would be two answers to one question. */
  var picker = null;

  function closePicker() {
    if (!picker) return;
    picker.remove();
    picker = null;
    document.removeEventListener('mousedown', pickerOutside, true);
    document.removeEventListener('keydown', pickerEscape, true);
  }

  function pickerOutside(event) {
    if (!picker) return;
    if (picker.contains(event.target)) return;
    // The chip that owns it handles its own click, which would otherwise close
    // and reopen in one gesture.
    if (event.target.closest && event.target.closest('.swatch-chip')) return;
    closePicker();
  }

  function pickerEscape(event) {
    // Escape only. Enter belongs to whatever field has the focus, and closing
    // on it would swallow a hex being committed in the box alongside.
    if (event.key === 'Escape') {
      closePicker();
      event.stopPropagation();
    }
  }

  /**
   * The popover for one swatch. `commit(hex, settle)` is the swatch's own
   * writer, so the field, the hue bar and a typed hex all end in exactly the
   * same place.
   *
   * `settle` is the difference between a pointer still down and a pointer let
   * go: everything follows the drag, the disk waits for the release. That
   * bargain is the whole reason this is one function and not three controls
   * wired separately.
   */
  function openPicker(cell, start, commit) {
    closePicker();

    var state = hsv(start);

    var box = document.createElement('div');
    box.className = 'picker';

    /*
     * The field. Two gradients over a flat hue: white to transparent across,
     * black to transparent down, which is exactly the HSV square and costs no
     * canvas and no redraw -- changing the hue changes one background colour.
     */
    var field = document.createElement('div');
    field.className = 'picker-field';
    field.tabIndex = 0;
    field.setAttribute('role', 'application');
    field.setAttribute('aria-label', text.pickerField);

    var thumb = document.createElement('div');
    thumb.className = 'picker-thumb';
    field.appendChild(thumb);

    var hue = document.createElement('input');
    hue.type = 'range';
    hue.className = 'picker-range picker-hue';
    hue.min = '0';
    hue.max = '359';
    hue.step = '1';
    hue.setAttribute('aria-label', text.pickerHue);

    var foot = document.createElement('div');
    foot.className = 'picker-foot';

    var shown = document.createElement('span');
    shown.className = 'picker-shown';

    var hex = document.createElement('input');
    hex.type = 'text';
    hex.className = 'picker-hex';
    hex.spellcheck = false;
    hex.maxLength = 7;
    hex.setAttribute('aria-label', text.pickerHex);

    foot.append(shown, hex);
    box.append(field, hue, foot);

    /** Everything the picker shows, from the one state it holds. */
    function paintPicker(typing) {
      var colour = fromHsv(state);
      field.style.backgroundColor = fromHsv([state[0], 1, 1]);
      thumb.style.left = (state[1] * 100) + '%';
      thumb.style.top = ((1 - state[2]) * 100) + '%';
      thumb.style.background = colour;
      hue.value = String(Math.round(state[0]));
      shown.style.background = colour;
      // Not while it is being typed into: rewriting the field under the caret
      // is how "#1a2" becomes unfinishable.
      if (!typing) hex.value = colour;
      return colour;
    }

    function set(next, settle, typing) {
      state = next;
      commit(paintPicker(typing), settle);
    }

    /* -- the field ---------------------------------------------------------- */

    function aim(event) {
      var box2 = field.getBoundingClientRect();
      var x = (event.clientX - box2.left) / box2.width;
      var y = (event.clientY - box2.top) / box2.height;
      set([state[0], Math.max(0, Math.min(1, x)), Math.max(0, Math.min(1, 1 - y))], false);
    }

    field.addEventListener('pointerdown', function (event) {
      // Captured, so a drag that leaves the square keeps painting instead of
      // stopping at the edge -- which is where the colour someone wants often
      // is, and letting go out there must still land it.
      field.setPointerCapture(event.pointerId);
      aim(event);
      event.preventDefault();
    });

    field.addEventListener('pointermove', function (event) {
      if (!field.hasPointerCapture(event.pointerId)) return;
      aim(event);
    });

    field.addEventListener('pointerup', function (event) {
      if (field.hasPointerCapture(event.pointerId)) field.releasePointerCapture(event.pointerId);
      set(state, true);
    });

    /*
     * The keyboard, on the one control that is not a native input. An arrow is
     * a percent and Shift is ten, which is the same pair of steps the hue bar
     * gets from Blink for free.
     */
    field.addEventListener('keydown', function (event) {
      var step = (event.shiftKey ? 10 : 1) / 100;
      var across = event.key === 'ArrowRight' ? step : event.key === 'ArrowLeft' ? -step : 0;
      var down = event.key === 'ArrowUp' ? step : event.key === 'ArrowDown' ? -step : 0;
      if (!across && !down) return;
      event.preventDefault();
      set([state[0], Math.max(0, Math.min(1, state[1] + across)),
        Math.max(0, Math.min(1, state[2] + down))], true);
    });

    /* -- the hue bar and the hex box ---------------------------------------- */

    hue.oninput = function () { set([Number(hue.value), state[1], state[2]], false); };
    hue.onchange = function () { set([Number(hue.value), state[1], state[2]], true); };

    hex.oninput = function () {
      var typed = hex.value.trim();
      var ok = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.test(typed);
      hex.classList.toggle('bad', !ok);
      if (!ok) return;
      set(hsv(toHex(typed)), false, true);
    };
    hex.onchange = function () {
      if (hex.classList.contains('bad')) return;
      set(state, true);
    };

    paintPicker();
    cell.appendChild(box);
    picker = box;
    document.addEventListener('mousedown', pickerOutside, true);
    document.addEventListener('keydown', pickerEscape, true);
    field.focus();
  }

  /*
   * The window's palette: eighteen colours per half, and what they add up to.
   *
   * The other thirty tokens theme.css defines are not here, because they are
   * not decisions -- `palette.js` computes them from these, and it computes
   * them only for a colour that was actually changed, so a half nobody touched
   * still wears the values that were measured by hand. That is the whole reason
   * the panel can be this short.
   *
   * Same bargain as the scheme editor below it: a colour being dragged is sent as
   * `themeApply`, which repaints the window and every frame in it and touches
   * no file; a colour let go is sent as `themeSave`, and what comes back is the
   * normalized file, which replaces the working copy.
   */
  /*
   * Which folds the user left open, so the page does not shut them again every
   * time it is drawn. Kept out of the config file on purpose: it is where the
   * scroll position was, not a preference, and it has no business travelling
   * between machines with the servers and the keys.
   */
  function rememberFolds() {
    document.querySelectorAll('details.fold[id]').forEach(function (fold) {
      var slot = 'tshell.fold.' + fold.id;
      try {
        if (localStorage.getItem(slot) === '1') fold.open = true;
      } catch (error) { /* private mode, or no storage: it stays shut. */ }
      fold.addEventListener('toggle', function () {
        try { localStorage.setItem(slot, fold.open ? '1' : '0'); }
        catch (error) { /* nothing to do: the fold still works. */ }
      });
    });
  }

  (function paletteEditor() {
    var api = window.tshellPalette;
    var row = document.querySelector('.palette-row');
    if (!api || !row) return;

    var halfBox = document.getElementById('palette-half');
    var resetButton = document.getElementById('palette-reset');
    var blind = document.getElementById('palette-blind');
    var errorBox = document.getElementById('palette-error');
    var preview = document.getElementById('palette-preview');
    var contrastBox = document.getElementById('palette-contrast');
    var fontField = document.getElementById('palette-font');
    var monoField = document.getElementById('palette-mono');
    var fontOptions = document.getElementById('palette-fonts');
    var monoOptions = document.getElementById('palette-monos');

    /*
     * Which block each group of tokens is drawn into.
     *
     * The accent used to sit with the text, on the argument that ink and the
     * colour under it are one question. It sits with the surfaces now, because
     * the question it actually answers is the one the whole first block asks --
     * this is a fill, what colour is it -- and because the ink that goes on it
     * is not a decision at all: `--ac-tx` is derived, and the contrast list
     * below reports it whether or not the swatch is nearby.
     */
    var boxes = {
      surface: document.getElementById('palette-surface'),
      text: document.getElementById('palette-ink'),
      meaning: document.getElementById('palette-meaning')
    };

    document.getElementById('palette-label').textContent = text.palette;
    document.getElementById('palette-hint').textContent = text.paletteHint;
    document.getElementById('palette-surface-title').textContent = text.paletteSurface;
    document.getElementById('palette-ink-title').textContent = text.paletteInk;
    document.getElementById('palette-meaning-title').textContent = text.paletteMeaning;
    document.getElementById('palette-font-label').textContent = text.paletteFont;
    document.getElementById('palette-mono-label').textContent = text.paletteMono;
    resetButton.textContent = text.paletteReset;
    fontField.placeholder = text.paletteFontDefault;
    monoField.placeholder = text.paletteMonoDefault;

    /* The name a person reads, for a token the stylesheet calls `--bg-dialog`. */
    function label(key) {
      return text['palette' + key.charAt(0).toUpperCase() + key.slice(1)] || key;
    }

    /*
     * The file that will not parse. Everything below still works -- the shipped
     * palette is a complete one -- but nothing is written while this is set.
     */
    var broken = boot.paletteError || '';
    if (broken) {
      errorBox.textContent = text.paletteBroken.replace('{0}', broken);
      errorBox.classList.add('c-notice-error');
      errorBox.hidden = false;
    }

    /* The working copy, and which half is being edited. */
    var file = api.normalize(boot.palette);
    var half = theme;

    function edits() {
      return file[half] || (file[half] = {});
    }

    function shipped(key) {
      return api.defaults(half)[key] || '#000000';
    }

    /** What this half actually paints with, edited or not. */
    function effective(key) {
      return edits()[key] || shipped(key);
    }

    var saveTimer = 0;

    /*
     * Repaint, without rebuilding.
     *
     * The same hazard the scheme editor documents: a picker is anchored to the
     * swatch that opened it, so redrawing the swatches mid-drag removes the
     * element being dragged. Only the two things that can change without
     * touching a control are redrawn here.
     */
    function apply() {
      vscode.postMessage({ type: 'themeApply', file: file });
      drawPreview();
      drawContrast();
    }

    function save() {
      clearTimeout(saveTimer);
      if (broken) return;
      vscode.postMessage({ type: 'themeSave', file: file });
    }

    function saveSoon() {
      apply();
      clearTimeout(saveTimer);
      saveTimer = setTimeout(save, 400);
    }

    /* -- the swatches ------------------------------------------------------- */

    function swatch(token) {
      var key = token.key;
      var set = edits()[key] || '';
      var shown = toHex(effective(key));

      var cell = document.createElement('div');
      cell.className = 'swatch' + (set ? '' : ' unset');

      var chip = document.createElement('button');
      chip.type = 'button';
      chip.className = 'swatch-chip';
      chip.style.background = shown;
      chip.setAttribute('aria-label', label(key));

      var name = document.createElement('span');
      name.className = 'swatch-name';
      name.textContent = label(key);
      // Narrow enough to elide is still narrow enough to read on hover, and at
      // some window width every one of these elides.
      name.title = label(key);

      var hex = document.createElement('input');
      hex.type = 'text';
      hex.className = 'swatch-hex';
      hex.value = set;
      // Empty is not "black", it is "whatever ships" -- so the field shows what
      // it falls through to rather than pretending nothing is there.
      hex.placeholder = shipped(key);
      hex.maxLength = 7;
      hex.spellcheck = false;
      hex.setAttribute('aria-label', label(key));

      function commit(value, settle) {
        edits()[key] = value;
        chip.style.background = value;
        hex.value = value;
        hex.classList.remove('bad');
        cell.classList.remove('unset');
        apply();
        if (settle) save();
      }

      chip.onclick = function () {
        if (picker && cell.contains(picker)) {
          closePicker();
          return;
        }
        openPicker(cell, edits()[key] || shown, commit);
      };

      hex.oninput = function () {
        var typed = hex.value.trim();
        if (!typed) {
          // Given up on purpose: this half has no opinion about this token any
          // more, so it goes back to following the stylesheet -- and so do the
          // tokens derived from it.
          delete edits()[key];
          hex.classList.remove('bad');
          cell.classList.add('unset');
          chip.style.background = shipped(key);
          apply();
          return;
        }
        var parsed = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.test(typed) ? toHex(typed) : '';
        hex.classList.toggle('bad', !parsed);
        if (!parsed) return;
        edits()[key] = parsed;
        chip.style.background = parsed;
        cell.classList.remove('unset');
        apply();
      };

      // This field and no other, for the reason the scheme editor gives: what
      // takes the focus away is usually the next chip along, and rebuilding here
      // would remove it between the press and the release.
      hex.onblur = function () {
        var held = edits()[key] || '';
        hex.value = held;
        hex.classList.remove('bad');
        cell.classList.toggle('unset', !held);
      };
      hex.onchange = save;

      cell.append(chip, name, hex);
      return cell;
    }

    /* -- what it adds up to -------------------------------------------------- */

    /*
     * The preview is painted from the derived palette, not inherited from the
     * page. That is what makes it the answer for the half the window is not
     * wearing: a light palette built inside a dark window has exactly one place
     * it can be seen, and this is it.
     */
    function drawPreview() {
      var full = api.resolved(file, half);

      preview.replaceChildren();
      preview.style.background = full['--bg-base'];
      preview.style.borderColor = full['--line-strong'];

      var stack = document.createElement('div');
      stack.className = 'pp-stack';

      var line = document.createElement('span');
      line.style.color = full['--tx'];
      line.textContent = text.paletteSample;

      var dim = document.createElement('span');
      dim.className = 'pp-dim';
      dim.style.color = full['--tx-dim'];
      dim.textContent = text.paletteSampleDim;

      stack.append(line, dim);
      preview.appendChild(stack);

      [
        [full['--ac'], full['--ac-tx'], label('ac')],
        [full['--err'], full['--err-tx'], label('err')],
        [full['--ac-soft'], full['--ac'], label('info')]
      ].forEach(function (fill) {
        var chip = document.createElement('span');
        chip.className = 'pp-fill';
        chip.style.background = fill[0];
        chip.style.color = fill[1];
        chip.textContent = fill[2];
        preview.appendChild(chip);
      });
    }

    /*
     * Measured, and only reported.
     *
     * `--tx-faint` is not checked: theme.css exempts it in writing because it is
     * icon strokes and hairlines and disabled controls, which WCAG exempts too.
     * Checking it would produce a warning that is always up, and a warning that
     * is always up is furniture.
     */
    var SURFACES = [
      'bgInput', 'bgCode', 'bgBase', 'bgElev',
      'bgTab', 'bgCard', 'bgDialog', 'bgMenu'
    ];

    /*
     * Both levels of readable text on every surface, and not on the four that
     * were listed back when there were six surfaces and four of them could not
     * be reached separately anyway. Every one of the eight is its own decision
     * now, which means every one of them can be taken somewhere `--tx-dim`
     * cannot be read.
     */
    var PAIRS = SURFACES.map(function (ground) { return ['tx', ground]; })
      .concat(SURFACES.map(function (ground) { return ['txDim', ground]; }))
      .concat([
        ['ac', 'bgBase'], ['ok', 'bgBase'], ['warn', 'bgBase'],
        ['err', 'bgBase'], ['info', 'bgBase']
      ]);

    function drawContrast() {
      var full = api.resolved(file, half);
      var bad = [];

      PAIRS.forEach(function (pair) {
        var ratio = api.contrast(effective(pair[0]), effective(pair[1]));
        if (ratio && ratio < 4.5) bad.push([label(pair[0]), label(pair[1]), ratio]);
      });

      /*
       * The three derived pairs. They are the ones nobody can see coming: the
       * ink on a fill is chosen by rule, and when the rule has nothing good to
       * choose from it is the fill underneath that has to move.
       */
      [['acTx', '--ac-tx', '--ac', 'ac'],
        ['errTx', '--err-tx', '--err', 'err'],
        ['chatUserTx', '--chat-user-tx', '--chat-user', 'chatUser']]
        .forEach(function (item) {
          var ratio = api.contrast(full[item[1]], full[item[2]]);
          if (ratio && ratio < 4.5) bad.push([label(item[0]), label(item[3]), ratio]);
        });

      contrastBox.replaceChildren();
      contrastBox.hidden = !bad.length;
      if (!bad.length) return;

      var head = document.createElement('div');
      head.textContent = text.paletteContrast;
      var list = document.createElement('ul');
      bad.forEach(function (item) {
        var entry = document.createElement('li');
        entry.textContent = text.paletteOn.replace('{0}', item[0]).replace('{1}', item[1])
          + ' — ' + item[2].toFixed(2) + ':1';
        list.appendChild(entry);
      });
      contrastBox.append(head, list);
    }

    /* -- drawing ------------------------------------------------------------- */

    /*
     * Tabs, not a segmented control.
     *
     * They looked the same and they are not the same thing. The theme switch
     * two rows above is a segmented control because pressing it changes the
     * window; this one changes which eighteen swatches are on screen and nothing
     * else -- press "light" while working in the dark theme and the window
     * stays dark, which is the whole point of it being a separate control. Worn
     * as the same filled pill, it read as a second theme switch that had failed
     * to take effect.
     *
     * `role="tab"` for the same reason, so what a screen reader is told matches
     * what the page is doing.
     */
    function drawHalfSwitch() {
      halfBox.replaceChildren();
      ['dark', 'light'].forEach(function (value) {
        var button = document.createElement('button');
        button.type = 'button';
        button.className = 'c-tab' + (half === value ? ' c-on' : '');
        button.textContent = text[value];
        button.setAttribute('role', 'tab');
        button.setAttribute('aria-selected', String(half === value));
        button.onclick = function () {
          if (half === value) return;
          half = value;
          draw();
        };
        halfBox.appendChild(button);
      });
    }

    function draw() {
      closePicker();
      drawHalfSwitch();

      boxes.surface.replaceChildren();
      boxes.text.replaceChildren();
      boxes.meaning.replaceChildren();
      api.tokens.forEach(function (token) {
        boxes[token.group].appendChild(swatch(token));
      });

      fontField.value = file.font || '';
      monoField.value = file.fontMono || '';

      blind.hidden = half === theme;
      if (!blind.hidden) {
        blind.textContent = text.paletteBlind
          .replace('{0}', text[half]).replace('{1}', text[theme]);
      }

      drawPreview();
      drawContrast();
    }

    /* -- the fields ---------------------------------------------------------- */

    fontField.oninput = function () { file.font = fontField.value; saveSoon(); };
    fontField.onchange = save;
    monoField.oninput = function () { file.fontMono = monoField.value; saveSoon(); };
    monoField.onchange = save;

    var askSeq = 0;
    var pending = new Map();

    /*
     * The shell draws its own dialogs -- the native ones freeze the WebView and
     * cannot follow the theme. Its own token space, so that this editor's
     * answers and the scheme editor's cannot be taken for each other.
     */
    function askShell(question) {
      return new Promise(function (resolve) {
        var token = 'p' + (askSeq += 1);
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

    resetButton.onclick = function () {
      if (!Object.keys(edits()).length) return;
      askShell({
        title: text.paletteResetTitle,
        message: text.paletteResetMessage.replace('{0}', text[half]),
        accept: text.paletteResetAccept
      }).then(function (ok) {
        if (!ok) return;
        file[half] = {};
        draw();
        apply();
        save();
      });
    };

    /* -- what comes back ----------------------------------------------------- */

    window.addEventListener('message', function (event) {
      var message = event.data || {};

      if (message.type === 'themeFile') {
        if (message.error) {
          broken = message.error;
          errorBox.textContent = text.paletteBroken.replace('{0}', broken);
          errorBox.classList.add('c-notice-error');
          errorBox.hidden = false;
        }
        /*
         * Adopted always, redrawn only when nothing here is in use. Every edit
         * sends a save and every save answers, so redrawing unconditionally
         * would destroy the control that made the edit -- the hex field loses
         * the keystroke, and a chip loses the picker hanging off it.
         */
        var busy = document.activeElement;
        var inUse = busy === fontField || busy === monoField
          || !!(busy && busy.closest && busy.closest('.swatch'));
        file = api.normalize(message.file);
        if (!inUse) draw();
        return;
      }

      /*
       * Both lists come from one scan on the other side. The monospaced one is
       * already on its way for the terminal's font field; this page only wants
       * it in a second datalist as well.
       */
      if (message.type === 'fonts') { fill(monoOptions, message.fonts); return; }
      if (message.type === 'fontsAll') { fill(fontOptions, message.fonts); return; }

      if (message.type === 'confirmed') {
        var resolve = pending.get(message.token);
        if (!resolve) return;
        pending.delete(message.token);
        resolve(!!message.ok);
      }
    });

    function fill(list, families) {
      list.replaceChildren();
      (families || []).forEach(function (family) {
        var option = document.createElement('option');
        option.value = family;
        list.appendChild(option);
      });
    }

    vscode.postMessage({ type: 'fontsAllRequest' });

    draw();

    /*
     * The theme switch moves what is being edited with it.
     *
     * Following rather than staying put, because the half in force is the half
     * in front of you, and wanting to change what you can see is the common
     * case by far. The other one is a click away and says so, on the line above
     * the swatches, for as long as it is in effect.
     */
    window.addEventListener('tshell:themechanged', function () {
      half = theme;
      draw();
    });
  }());

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
  /*
   * The shortcut editor.
   *
   * Small because the rule it enforces is not here: `shared/keys.js` owns what
   * a binding may be, and it is the same file `host.js` matches keystrokes
   * with. This page asks, shows and stores.
   *
   * What is edited is the user's *edits*, not the table: a row left alone is
   * absent from the file, so a later build that picks a better default hands it
   * to everyone who never opened this section. `resolve` is what turns the two
   * into something to draw.
   */
  (function keyEditor() {
    var rowsBox = document.getElementById('keys-rows');
    var errorBox = document.getElementById('keys-error');
    var resetButton = document.getElementById('keys-reset');
    if (!rowsBox || !window.tshellKeys) return;

    var api = window.tshellKeys;
    var file = api.normalize(boot.keys);
    var capturing = '';

    document.getElementById('keys-title').textContent = text.keysTitle;
    document.getElementById('keys-hint').textContent = text.keysHint;
    resetButton.textContent = text.keysReset;

    function label(id) {
      return text['keys' + id.charAt(0).toUpperCase() + id.slice(1)] || id;
    }

    function complain(message) {
      errorBox.textContent = message;
      errorBox.classList.add('c-notice-error');
      errorBox.hidden = !message;
    }

    function save() {
      vscode.postMessage({ type: 'keysSave', keys: file });
    }

    /*
     * Leave capture, whether or not anything was bound. Always paired with the
     * listener below and with the bridge being stood back up: a page that
     * returned from capture without clearing that flag would be a window whose
     * shortcuts had quietly stopped working until it was reloaded.
     */
    function stopCapture() {
      capturing = '';
      window.tshellShortcutsPaused = false;
      window.removeEventListener('keydown', onCapture, true);
      draw();
    }

    function onCapture(event) {
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) return;

      // Escape is the way out, so it is not a thing that can be bound here.
      if (event.code === 'Escape') {
        complain('');
        stopCapture();
        return;
      }

      var pressed = api.fromEvent(event);
      // A modifier on its own -- the user is still on their way somewhere.
      if (!pressed) return;

      var why = api.usable(pressed);
      if (why) {
        complain(text[why].replace('{0}', api.label(pressed) || pressed));
        return;
      }

      var resolved = api.resolve(file);
      var clash = '';
      Object.keys(resolved).forEach(function (other) {
        if (other !== capturing && resolved[other] === pressed) clash = other;
      });
      if (clash) {
        complain(text.keysTaken.replace('{0}', api.label(pressed)).replace('{1}', label(clash)));
        return;
      }

      /*
       * Back to the default is stored as nothing at all, which `normalize`
       * already does -- so this assigns and lets it drop rather than testing
       * for it here. Two places that know what a default is would be one too
       * many.
       */
      file[capturing] = pressed;
      file = api.normalize(file);
      complain('');
      stopCapture();
      save();
    }

    function startCapture(id) {
      if (capturing) stopCapture();
      capturing = id;
      complain('');
      /*
       * The bridge is holding a listener that would take this keystroke and
       * act on it -- which, in a panel asking what the keystroke should do, is
       * the one answer that is not allowed.
       */
      window.tshellShortcutsPaused = true;
      window.addEventListener('keydown', onCapture, true);
      draw();
    }

    function draw() {
      var resolved = api.resolve(file);
      rowsBox.replaceChildren();
      api.actions.forEach(function (action) {
        var row = document.createElement('div');
        row.className = 'row';

        var name = document.createElement('div');
        name.className = 'label';
        var title = document.createElement('span');
        title.textContent = label(action.id);
        var hint = document.createElement('span');
        hint.className = 'hint';
        hint.textContent = text.keysChange;
        name.append(title, hint);

        var button = document.createElement('button');
        button.type = 'button';
        button.className = 'c-btn keys-binding';
        var live = capturing === action.id;
        button.classList.toggle('capturing', live);
        button.textContent = live ? text.keysCapturing : api.label(resolved[action.id]);
        button.onclick = function () {
          if (capturing === action.id) stopCapture();
          else startCapture(action.id);
        };

        row.append(name, button);
        rowsBox.appendChild(row);
      });
    }

    resetButton.onclick = function () {
      if (capturing) stopCapture();
      file = {};
      complain('');
      draw();
      save();
    };

    /*
     * Redrawn from what was stored, not from what was sent. A binding this
     * build wrote and Rust would not keep is a binding the panel must stop
     * showing -- otherwise the row says a key is bound and nothing ever fires.
     */
    window.addEventListener('message', function (event) {
      var data = event.data || {};
      if (data.type !== 'keysFile') return;
      file = api.normalize(data.keys);
      if (data.error) complain(String(data.error));
      draw();
    });

    draw();
  })();

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
    var currentName = document.getElementById('scheme-current');

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
      errorBox.classList.add('c-notice-error');
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

      // A button, not `<input type="color">`. See `openPicker` for why.
      var chip = document.createElement('button');
      chip.type = 'button';
      chip.className = 'swatch-chip';
      chip.style.background = shown;
      chip.setAttribute('aria-label', key);

      var name = document.createElement('span');
      name.className = 'swatch-name';
      name.textContent = key;
      name.title = key;

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

      /*
       * The one place a colour is written, whichever control asked for it.
       * `settle` is the difference between a slider mid-drag and a slider let
       * go: the terminals follow every frame, the disk waits for the release.
       */
      function commit(value, settle) {
        colorRecord()[key] = value;
        chip.style.background = value;
        hex.value = value;
        hex.classList.remove('bad');
        cell.classList.remove('unset');
        apply();
        if (settle) save();
      }

      chip.onclick = function () {
        if (picker && cell.contains(picker)) {
          closePicker();
          return;
        }
        openPicker(cell, storedColors()[key] || shown, commit);
      };

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
      // Every swatch below is about to be replaced, and a picker anchored to one
      // of them would be left pointing at an element that no longer exists.
      closePicker();
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

      // Which scheme -- the one thing a shut row has to carry, and the one
      // thing "terminal appearance" on its own cannot say.
      currentName.textContent = pick.options[pick.selectedIndex]
        ? pick.options[pick.selectedIndex].textContent
        : '';

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
          errorBox.classList.add('c-notice-error');
          errorBox.hidden = false;
          return;
        }
        broken = '';
        errorBox.hidden = true;
        /*
         * Not while any control in this editor is in use. Rust's answer is the
         * same value spelled its way, and swapping the field out from under
         * someone mid-word would move their cursor to the end of it.
         *
         * The swatches matter more than the three named fields, not less. Those
         * are static and merely lose their caret; a swatch is rebuilt wholesale
         * by `draw`, so redrawing while one has the focus REMOVES the element
         * being used -- and every edit sends a save, so every edit came back and
         * destroyed the control that made it. Typing into a hex field lost the
         * keystroke, and a colour chip could not open its picker at all, because
         * the input owning that native dialog was gone between the press and the
         * release. `hex.onblur` guards against exactly this and says so; this is
         * the same hazard arriving by the other road.
         */
        var busy = document.activeElement;
        var inUse = busy === nameField || busy === fontField || busy === sizeField
          || !!(busy && busy.closest && busy.closest('.swatch'));
        if (inUse) {
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
    var next = document.body.classList.contains('vscode-light') ? 'light' : 'dark';
    /*
     * The body's class list is not only the theme, and reacting to all of it
     * makes both editors below unusable.
     *
     * `scrollbars.js` marks whatever the pointer is over that can scroll with
     * `c-scroll-hot`, and on this page the thing that scrolls is the body
     * itself -- so the class list changes as the pointer enters and leaves the
     * page, several times a second while anyone is using it. Every one of those
     * used to arrive here as "the theme changed" and redraw both editors, which
     * rebuild their swatches wholesale. The element under the pointer was then
     * being replaced between the press and the release: no click event ever
     * fired, a colour chip could not open its picker, the half switch would not
     * switch, and a hex field lost the focus the moment it took it.
     *
     * So the comparison, not just the read. What is watched is a class list;
     * what is announced is a change of theme.
     *
     * Compared against `wearing` and not against `theme`, because the two are
     * not the same question. The control above sets `theme` the instant it is
     * clicked, so that the switch redraws under the finger while the shell is
     * still being told; `wearing` is what the body's class actually says. If
     * this compared `theme`, the user's own click would be the one change that
     * arrived here already accounted for -- and the editors would never hear
     * about the theme the user just chose, which is the case this observer
     * exists for.
     */
    if (next === wearing) return;
    wearing = next;
    theme = next;
    paint();
    // The editors below show the half of a scheme or a palette that is in use,
    // and read colours out of the stylesheet. Both are true only now, once the
    // class is actually on the body.
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
      terminal: '把终端最近输入输出发给模型',
      terminalHint: '你敲过的命令和它们的输出。发之前会做脱敏，但这仍然是把屏幕内容交出去；关掉则一个字都不发',
      lines: '最多发多少行',
      linesHint: '只在上面那项开着时有意义',
      memory: '启用记忆',
      memoryHint: '关掉后不注入已记内容，也不再给模型 remember 动作',
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
      terminal: 'Send recent terminal input and output',
      terminalHint: 'The commands you typed and what they printed. Masked first, but this is still handing over what is on your screen; off sends none of it',
      lines: 'Lines to send',
      linesHint: 'Only meaningful while the setting above is on',
      memory: 'Use memory',
      memoryHint: 'Off injects nothing and stops offering the model remember',
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
  // After both editors have drawn: opening a fold whose contents do not exist
  // yet is a fold that opens onto nothing.
  rememberFolds();
  vscode.postMessage({ type: 'aiRead' });
})();
