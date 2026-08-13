/*
 * The syntax highlighter shared by the file preview, the file-change cards and
 * the chat panel's code blocks, so a snippet looks the same wherever tshell
 * shows it.
 *
 * Everything here returns an HTML string rather than nodes, and every literal
 * that reaches the output goes through escapeHtml on the way. That is what makes
 * the result safe to assign to innerHTML, which is the only reason a highlighter
 * can be this small: the alternative is building a span per token by hand.
 *
 * It is a line highlighter, not a parser. Each line is classified on its own,
 * with a small state object carrying the two things a line genuinely cannot know
 * by itself -- whether a block comment or a Python docstring is still open. That
 * ceiling is deliberate: a real grammar per language is an order of magnitude
 * more code, and everything above this line only ever shows short excerpts.
 *
 * The token names follow what an editor calls them rather than what the regex
 * matched, because the colours are VS Code's: comments green, control flow
 * magenta, other keywords blue, types teal, calls yellow, strings orange.
 */
(function () {
  'use strict';

  function escapeHtml(value) {
    return String(value === undefined || value === null ? '' : value).replace(/[&<>"']/g, (char) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
    })[char]);
  }

  function span(className, value) {
    return '<span class="' + className + '">' + escapeHtml(value) + '</span>';
  }

  function words(text) {
    return new Set(text.split(/\s+/).filter(Boolean));
  }

  // -- languages ------------------------------------------------------------

  /*
   * One entry per language rather than one union of every keyword there is. The
   * union was cheaper and wrong in a way that showed: `var` lit up inside every
   * /var/log path in a shell script, and `in`, `is` and `with` did the same to
   * ordinary prose in a config file.
   *
   * `control` is the flow of the program -- if, for, return -- and is coloured
   * apart from `keyword`, which is everything declarative. That split is the
   * single biggest reason highlighted code reads as structure rather than as
   * confetti, and it is what every modern editor theme does.
   */
  const LANGUAGES = {};

  function define(names, spec) {
    const entry = {
      control: words(spec.control || ''),
      keywords: words(spec.keywords || ''),
      types: words(spec.types || ''),
      comment: spec.comment || '',
      block: Boolean(spec.block),
      backtick: Boolean(spec.backtick),
      classCase: Boolean(spec.classCase)
    };
    names.split(' ').forEach((name) => { LANGUAGES[name] = entry; });
  }

  define('c cpp', {
    control: 'break case catch continue default do else for goto if return switch throw try while '
      + 'co_await co_return co_yield',
    keywords: 'alignas alignof and asm auto class concept const consteval constexpr constinit decltype delete '
      + 'dynamic_cast explicit export extern false friend inline module mutable namespace new noexcept not '
      + 'nullptr operator or private protected public register reinterpret_cast requires sizeof static '
      + 'static_assert static_cast struct template this thread_local true typedef typeid typename union using '
      + 'virtual volatile enum',
    types: 'bool char char8_t char16_t char32_t double float int int8_t int16_t int32_t int64_t long short '
      + 'signed size_t ssize_t string std uint8_t uint16_t uint32_t uint64_t unsigned void wchar_t FILE '
      + 'vector map unordered_map deque array pair',
    comment: 'c',
    block: true
  });

  define('java', {
    control: 'break case catch continue default do else finally for if return switch throw try while yield',
    keywords: 'abstract assert class const enum extends final implements import instanceof interface native new '
      + 'package private protected public record sealed static strictfp super synchronized this throws '
      + 'transient var volatile true false null',
    types: 'boolean byte char double float int long short void String Integer Long Double Boolean Object List '
      + 'Map Set Exception',
    comment: 'c',
    block: true,
    classCase: true
  });

  define('javascript typescript', {
    control: 'await break case catch continue default do else finally for if return switch throw try while yield',
    keywords: 'abstract as async class const declare delete export extends from function get import in '
      + 'instanceof interface let namespace new of private protected public readonly satisfies set static super '
      + 'this typeof var void with true false null undefined',
    types: 'any bigint boolean never number object string symbol unknown Array Promise Record Map Set Date '
      + 'RegExp Error Buffer',
    comment: 'c',
    block: true,
    backtick: true,
    classCase: true
  });

  define('python', {
    control: 'break case continue elif else except finally for if match raise return try while with yield',
    keywords: 'and as assert async await class def del from global import in is lambda nonlocal not or pass '
      + 'self cls None True False',
    types: 'bool bytes bytearray complex dict float frozenset int list object set str tuple type Exception '
      + 'ValueError TypeError KeyError OSError',
    comment: 'hash',
    classCase: true
  });

  define('shell', {
    control: 'break case continue do done elif else esac fi for if in return select then until while',
    keywords: 'alias declare eval exec export function local readonly set shift source trap unset wait exit '
      + 'true false',
    types: '',
    comment: 'hash'
  });

  define('css', {
    control: '',
    keywords: 'important from to and not only',
    types: '',
    comment: 'c',
    block: true
  });

  /** Everything the tables above do not name: strings, numbers and comments only. */
  const PLAIN = {
    control: words(''), keywords: words(''), types: words(''),
    comment: '', block: false, backtick: false, classCase: false
  };

  function specFor(language) {
    return LANGUAGES[language] || PLAIN;
  }

  // -- tokens ---------------------------------------------------------------

  const NUMBER = /^(?:0[xX][\da-fA-F]+|0[bB][01]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?[uUlLfF]*)$/;
  const IDENTIFIER = /^[A-Za-z_$][\w$]*$/;

  /*
   * Matched in one pass so that `$PATH`, `@decorator` and a bare word cannot be
   * split across two rules and coloured twice. Order inside the alternation is
   * the precedence: the longest, most distinctive shape first.
   */
  const TOKEN = /(\$\{[^}]*\}|\$[A-Za-z_@#?*!$0-9-]+|@[A-Za-z_][\w.]*|0[xX][\da-fA-F]+|0[bB][01]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?[uUlLfF]*|[A-Za-z_$][\w$]*)/g;

  function classifyToken(token, nextChar, spec) {
    if (token[0] === '$') return span('tok-variable', token);
    if (token[0] === '@') return span('tok-decorator', token);
    if (NUMBER.test(token)) return span('tok-number', token);
    if (spec.control.has(token)) return span('tok-control', token);
    if (spec.keywords.has(token)) return span('tok-keyword', token);
    if (spec.types.has(token)) return span('tok-type', token);
    // A call is named by what follows it, which is the one piece of context a
    // line highlighter reliably has.
    if (nextChar === '(' && IDENTIFIER.test(token)) return span('tok-function', token);
    if (spec.classCase && /^[A-Z][\w$]*$/.test(token) && /[a-z]/.test(token)) return span('tok-type', token);
    return escapeHtml(token);
  }

  function highlightWords(value, spec) {
    let result = '';
    let last = 0;
    for (const match of value.matchAll(TOKEN)) {
      const index = match.index || 0;
      result += escapeHtml(value.slice(last, index));
      const after = value.slice(index + match[0].length).match(/^\s*(.)/);
      result += classifyToken(match[0], after ? after[1] : '', spec);
      last = index + match[0].length;
    }
    return result + escapeHtml(value.slice(last));
  }

  /** Strings first, so nothing inside one is ever read as code. */
  function highlightCode(value, spec) {
    const pattern = spec.backtick
      ? /("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)/g
      : /("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')/g;
    let result = '';
    let last = 0;
    for (const match of value.matchAll(pattern)) {
      const index = match.index || 0;
      result += highlightWords(value.slice(last, index), spec);
      result += span('tok-string', match[0]);
      last = index + match[0].length;
    }
    return result + highlightWords(value.slice(last), spec);
  }

  // -- per-family lines -----------------------------------------------------

  /*
   * A `#` or `//` inside a string is not a comment, so the line is cut at the
   * first marker that no quote covers. Counting quotes is enough for real code
   * and cheap; what it cannot see is a quote inside a comment, which is harmless
   * because everything after the marker is comment-coloured anyway.
   */
  function commentStart(line, marker) {
    let quote = '';
    for (let index = 0; index < line.length; index += 1) {
      const char = line[index];
      if (quote) {
        if (char === '\\') index += 1;
        else if (char === quote) quote = '';
        continue;
      }
      if (char === '"' || char === "'" || char === '`') { quote = char; continue; }
      if (line.startsWith(marker, index)) return index;
    }
    return -1;
  }

  function withLineComment(line, marker, spec) {
    const at = commentStart(line, marker);
    if (at < 0) return highlightCode(line, spec);
    return highlightCode(line.slice(0, at), spec) + span('tok-comment', line.slice(at));
  }

  function highlightCStyleLine(line, spec, state) {
    if (state.blockComment) {
      const end = line.indexOf('*/');
      if (end < 0) return span('tok-comment', line);
      state.blockComment = false;
      return span('tok-comment', line.slice(0, end + 2)) + highlightCStyleLine(line.slice(end + 2), spec, state);
    }
    const blockStart = line.indexOf('/*');
    const lineComment = commentStart(line, '//');
    const commentAt = blockStart >= 0 && (lineComment < 0 || blockStart < lineComment) ? blockStart : lineComment;
    if (commentAt >= 0) {
      const before = line.slice(0, commentAt);
      const comment = line.slice(commentAt);
      if (comment.startsWith('/*')) {
        const end = comment.indexOf('*/', 2);
        if (end < 0) {
          state.blockComment = true;
          return highlightCode(before, spec) + span('tok-comment', comment);
        }
        return highlightCode(before, spec) + span('tok-comment', comment.slice(0, end + 2))
          + highlightCStyleLine(comment.slice(end + 2), spec, state);
      }
      return highlightCode(before, spec) + span('tok-comment', comment);
    }
    // `#include <vector>`: the directive is a keyword and the header is a string,
    // which is how an editor shows it and why the angle brackets are not markup.
    const directive = line.match(/^(\s*#\s*[a-z_]+)(\s*)(<[^>]*>)?(.*)$/);
    if (directive) {
      return escapeHtml(directive[1].slice(0, directive[1].length - directive[1].trimStart().length))
        + span('tok-preprocessor', directive[1].trimStart())
        + escapeHtml(directive[2])
        + (directive[3] ? span('tok-string', directive[3]) : '')
        + highlightCode(directive[4], spec);
    }
    return highlightCode(line, spec);
  }

  /*
   * Python's triple-quoted strings are the reason this language needs a line of
   * its own. A module docstring is prose, and run through the ordinary path every
   * "if" and "for" in it comes out as a keyword -- which is exactly what a file
   * opening with one looks like. So the fence is tracked across lines, the same
   * way a C block comment is.
   */
  function highlightPythonLine(line, spec, state) {
    if (state.docstring) {
      const end = line.indexOf(state.docstring);
      if (end < 0) return span('tok-string', line);
      const quote = state.docstring;
      state.docstring = '';
      return span('tok-string', line.slice(0, end + quote.length))
        + highlightPythonLine(line.slice(end + quote.length), spec, state);
    }
    const open = line.match(/"""|'''/);
    const hash = commentStart(line, '#');
    if (!open || (hash >= 0 && hash < open.index)) return withLineComment(line, '#', spec);

    const before = line.slice(0, open.index);
    const rest = line.slice(open.index + 3);
    const end = rest.indexOf(open[0]);
    if (end < 0) {
      state.docstring = open[0];
      return withLineComment(before, '#', spec) + span('tok-string', line.slice(open.index));
    }
    return withLineComment(before, '#', spec)
      + span('tok-string', open[0] + rest.slice(0, end + 3))
      + highlightPythonLine(rest.slice(end + 3), spec, state);
  }

  function highlightIniLine(line) {
    if (/^\s*[;#]/.test(line)) return span('tok-comment', line);
    const section = line.match(/^(\s*)\[([^\]]+)\](.*)$/);
    if (section) return escapeHtml(section[1]) + span('tok-section', '[' + section[2] + ']') + escapeHtml(section[3]);
    const pair = line.match(/^(\s*)([^=:#\s][^=:#]*?)(\s*[=:])(.*)$/);
    if (pair) {
      return escapeHtml(pair[1]) + span('tok-key', pair[2].trimEnd())
        + escapeHtml(pair[2].slice(pair[2].trimEnd().length)) + span('tok-operator', pair[3])
        + highlightCode(pair[4], PLAIN);
    }
    return escapeHtml(line);
  }

  /*
   * A JSON key is a string in a place that makes it a name, and reads better as
   * one. What settles it is the colon after it, so every string on the line is
   * looked at rather than only the first -- a whole object often arrives on one
   * line, and the second key is a key for the same reason the first one is.
   */
  function highlightJsonLine(line) {
    const spec = specFor('javascript');
    let result = '';
    let last = 0;
    for (const match of line.matchAll(/"(?:\\.|[^"\\])*"/g)) {
      const index = match.index || 0;
      result += highlightWords(line.slice(last, index), spec);
      result += span(/^\s*:/.test(line.slice(index + match[0].length)) ? 'tok-key' : 'tok-string', match[0]);
      last = index + match[0].length;
    }
    return result + highlightWords(line.slice(last), spec);
  }

  function highlightMarkupLine(line) {
    return escapeHtml(line)
      .replace(/([\w:-]+)=(&quot;.*?&quot;|&#39;.*?&#39;)/g,
        '<span class="tok-attr">$1</span>=<span class="tok-string">$2</span>')
      .replace(/(&lt;\/?[\w:-]+)/g, '<span class="tok-tag">$1</span>');
  }

  function highlightLine(line, language, state) {
    const name = String(language || '');
    if (name === 'ini') return highlightIniLine(line);
    if (name === 'json') return highlightJsonLine(line);
    if (name === 'xml' || name === 'html') return highlightMarkupLine(line);

    const spec = specFor(name);
    if (name === 'python') return highlightPythonLine(line, spec, state || newState());
    if (spec.block) return highlightCStyleLine(line, spec, state || newState());
    if (spec.comment === 'hash') return withLineComment(line, '#', spec);
    return highlightCode(line, spec);
  }

  /*
   * Carries what a line cannot know by itself from one line to the next: whether
   * a C block comment or a Python triple-quoted string is still open.
   */
  function newState() {
    return { blockComment: false, docstring: '' };
  }

  /*
   * Markdown fences name a language in whatever way the model felt like, while
   * the file preview names it after the extension it recognised. This maps the
   * former onto the latter. An unknown name returns '', which callers read as
   * "leave it alone" rather than guessing at C.
   */
  const ALIASES = {
    sh: 'shell', bash: 'shell', zsh: 'shell', shell: 'shell', console: 'shell', terminal: 'shell',
    py: 'python', python: 'python',
    c: 'c', h: 'c',
    cpp: 'cpp', 'c++': 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp',
    java: 'java',
    js: 'javascript', javascript: 'javascript', node: 'javascript',
    ts: 'typescript', typescript: 'typescript',
    json: 'json',
    css: 'css',
    html: 'html', htm: 'html', xml: 'xml',
    ini: 'ini', conf: 'ini', cfg: 'ini', toml: 'ini', properties: 'ini', env: 'ini',
    yml: 'ini', yaml: 'ini'
  };

  function normalizeLanguage(name) {
    return ALIASES[String(name || '').trim().toLowerCase()] || '';
  }

  /*
   * What an untagged fence is probably written in.
   *
   * A model that writes ``` on its own, or tags a block "text" or "output", leaves
   * a page of C++ looking like grey prose. Guessing from the content fixes that,
   * but only where the opening is unmistakable: a shebang, a #include, a def. Each
   * test below is one a paragraph of English cannot pass by accident, and anything
   * that matches none of them returns '' and is left alone -- a wrong guess paints
   * the whole block in the wrong colours, which is worse than no colour at all.
   */
  const SIGNS = [
    [/^#!.*\bpython/m, 'python'],
    [/^#!.*\b(?:ba|z|k|da)?sh\b/m, 'shell'],
    [/^\s*#\s*(?:include|pragma\s+once)\b/m, 'c'],
    [/^\s*(?:def|class)\s+\w+.*:\s*$/m, 'python'],
    [/^\s*(?:from\s+[\w.]+\s+import\s|import\s+[\w.]+\s*$)/m, 'python'],
    [/^\s*if\s+__name__\s*==/m, 'python'],
    [/^\s*(?:package\s+[\w.]+;|public\s+(?:final\s+|abstract\s+)?class\s+\w)/m, 'java'],
    [/^\s*(?:function\s+\w+\s*\(|(?:const|let|var)\s+\w+\s*=|module\.exports\b|export\s+(?:default|const|function)\s)/m, 'javascript'],
    [/^\s*(?:interface\s+\w+\s*\{|type\s+\w+\s*=)/m, 'typescript'],
    [/^\s*\{\s*$|^\s*"[\w.-]+"\s*:/m, 'json'],
    [/^\s*<[a-zA-Z!?/]/m, 'html'],
    [/^\s*\[[\w.$ -]+\]\s*$/m, 'ini'],
    [/^\s*(?:\$\s+\S|sudo|apt(?:-get)?|yum|dnf|systemctl|journalctl|docker|kubectl|git|make|chmod|chown|mkdir|rm|cp|mv|tar|curl|wget|ssh|scp|grep|awk|sed|cat|ls|cd|echo|export|source)\s+\S/m, 'shell']
  ];

  function guessLanguage(source) {
    const text = String(source || '');
    if (!text.trim()) return '';
    for (const [pattern, language] of SIGNS) {
      if (!pattern.test(text)) continue;
      // The C family shares one opening. Anything only C++ has settles which half.
      if (language === 'c') return /\bstd::|\btemplate\s*<|\bnamespace\b|\bclass\s+\w|<iostream>|<vector>|<string>/.test(text) ? 'cpp' : 'c';
      return language;
    }
    return '';
  }

  /*
   * The name to hand highlightLine for a file, which is its extension where that
   * says something and its content where it does not -- a shell script called
   * `deploy` has no extension to read.
   */
  function languageForFile(path, content) {
    const name = String(path || '').split(/[\\/]/).pop() || '';
    const dot = name.lastIndexOf('.');
    return (dot > 0 ? normalizeLanguage(name.slice(dot + 1)) : '') || guessLanguage(content);
  }

  /** The whole of a snippet, ready for innerHTML. */
  function highlightText(text, language) {
    const state = newState();
    return String(text === undefined || text === null ? '' : text)
      .split('\n')
      .map((line) => highlightLine(line, language, state))
      .join('\n');
  }

  window.tshellHighlight = {
    escapeHtml: escapeHtml,
    guessLanguage: guessLanguage,
    highlightLine: highlightLine,
    highlightText: highlightText,
    languageForFile: languageForFile,
    newState: newState,
    normalizeLanguage: normalizeLanguage
  };
}());
