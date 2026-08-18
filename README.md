# tshell

AI assistant working in your own SSH terminal, with SSH/SFTP sessions and
dual-pane file transfer. No server-side setup.

Windows / Linux / macOS desktop application, built on Tauri v2 with a Rust
backend. 

## Build

```bash
cd src-tauri
cargo build -j 2
./target/debug/tshell.exe
```

Installers come from `npm install && npm run build` (`tauri build`), which
bundles for the host platform.

Two things that are easy to get wrong:

- **The frontend is embedded at compile time.** Editing `ui/` and re-running the
  binary changes nothing; you have to `cargo build` again.
- **Rust does not check the frontend.** After editing `ui/`, run `node --check`
  over the files you touched. A syntax error there is invisible until the page
  opens blank.

`rust-version` in `Cargo.toml` must match the toolchain in use. Setting it lower
is not free: cargo's MSRV-aware resolver holds every dependency back to a
version compatible with it, and some of those old versions no longer build on a
current rustc.

## Layout

```
src-tauri/          Rust: the whole backend, and the only thing that touches the OS
  src/
    main.rs         command surface and the boundary translation to the pages
    config.rs       servers, groups, normalisation, atomic read/write
    secrets.rs      the system keychain, and the fallback when there isn't one
    ssh.rs          one channel per session; bytes to the terminal
    hosts.rs        host keys: pinned on first use, judged, asked about
    transfer.rs     both sides behind one abstraction; the transfer job
    preview.rs      chunked text and DBF
    schemes.rs      terminal appearance: version gate, hex normalisation
    theme.rs        the window's palette, as the user's edits to it
    fonts.rs        font enumeration (the web platform cannot be asked)
    i18n.rs         reads ui/vendor/strings.json with include_str!
    atomic.rs       temp file + rename
    ai/             the agent loop, the providers, the policies, the five stores
      tools.rs      the tool table, and a call turned into an action
      link.rs       the assistant's own SFTP connection; bytes, never commands
  tauri.conf.json   frameless window; frontendDist points at ../ui
ui/                 the pages -- plain HTML/CSS/JS, no build step
  shell/            the window shell; the only IPC exit in the window
  shared/           theme.css, palette.js, components.css, icons.js,
                    schemes.js, host.js, dropdown.js
  servers/ terminal/ transfer/ chat/ settings/
  vendor/           xterm, the icon, and strings.json
```

## The rules that hold it together

1. **Pages never touch Tauri.** page → shell (`postMessage`) → Rust (`invoke`).
   The shell is the window's single IPC exit.
2. **Every iframe is a direct child of `.panes`**, from creation to destruction;
   changing columns only changes `grid-column`. Re-parenting an iframe discards
   and rebuilds its browsing context -- SSH reconnects, scrollback is lost.
3. **Bootstrap data rides the URL fragment, not a message.** A page reads it
   synchronously at the top of its own script, before any message could arrive.
4. **The theme is set in two places**: `data-theme` on `<html>` drives the CSS
   variables; `vscode-light`/`vscode-dark` on `<body>` is what `terminal.js`'s
   MutationObserver and `highlight.css` watch.
5. **Terminal colours can only come from JS.** xterm draws to a canvas, where CSS
   variables do not reach, so its palette lives in `schemes.js`. Colours both the
   terminal and the window need are still written once in `theme.css` and read
   with `getPropertyValue` from both sides.
6. **`--tx-faint` never carries text.** It is for icon strokes, hairlines and
   disabled states; it does not reach 4.5:1.
7. **A host key is pinned on first use and questioned after that.** The rules
   live in `hosts.rs`; `ssh.rs` only translates russh's key type into them. The
   question travels out as an event and comes back through `host_key_answer`,
   because the connection that raises it may belong to no command at all.
8. **The model protocol has two tracks and both are permanent.** Tools are
   offered on every request; an endpoint that rejects the field gets the request
   again without it and answers in the JSON protocol the prompt still describes.
   A call becomes an action by putting the verb back into its arguments and
   handing the result to `parse::action_from_object` -- the same function the
   JSON track ends in, so the two cannot drift.
9. **The palette is editable, but only fifteen tokens of it per theme.** The
   other thirty are derived in `ui/shared/palette.js`, and derived only for a
   token that was actually changed -- so a colour nobody edited keeps the value
   that was measured by hand, and a later build can retune the shipped palette
   for everyone who never opened the panel. `node scripts/palette-check.mjs`
   holds `theme.rs`, `palette.js` and `theme.css` to one list of tokens and to
   4.5:1.
10. **The assistant's own connection moves bytes; it never runs commands.**
   Commands go through the user's terminal, which is the whole product. `link.rs`
   carries file content, and the commit stays a shell command so the move is
   still atomic and the user still sees what was written.
11. **Every control is written down once, in `ui/shared/components.css`.** A
   page opts in by putting `c-btn` on a button; nothing there applies by
   accident. The chat panel spent a long time not doing this -- it had buttons,
   fields, dialogs and list rows of its own, meaning the same things -- and they
   drifted exactly where you would expect: a border at 11% opacity in one place
   and 6% in the other, three different marks for "the row in force", eleven
   font sizes on a thirteen-pixel body. None of it is visible in any one panel.
   It is visible when you move between them, which is the whole window.
12. **A dropdown's closed control is the platform's; its list is ours.** The
   `<select>` and the `<input list>` stay in the page holding the value, so
   `.value`, `.onchange`, the label association and what a screen reader is told
   all still come from the browser. `shared/dropdown.js` replaces only the popup
   -- because Chromium draws a select's and a datalist's with two different
   pieces of code, agreeing on neither radius, row height, shadow nor highlight,
   and neither of them can be styled.

`ui/vendor/strings.json` is the one string table -- 341 entries, two languages.
The pages read it because they have no host to ask, and Rust reads the same file
with `include_str!`. Edit it directly; there is no generator upstream of it any
more.

## Licence

MIT. See `LICENSE`.
