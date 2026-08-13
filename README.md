# tshell

AI assistant working in your own SSH terminal, with SSH/SFTP sessions and
dual-pane file transfer. No server-side setup.

Windows / Linux / macOS desktop application, built on Tauri v2 with a Rust
backend. It began as the VS Code extension **tshell 2.2.1**; that codebase is a
separate repository and is no longer maintained. `MIGRATION.md` records what was
carried over, what was deliberately dropped, and the reasoning behind both.

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
    transfer.rs     both sides behind one abstraction; the transfer job
    preview.rs      chunked text and DBF
    schemes.rs      terminal appearance: version gate, hex normalisation
    fonts.rs        font enumeration (the web platform cannot be asked)
    i18n.rs         reads ui/vendor/strings.json with include_str!
    atomic.rs       temp file + rename
    ai/             the agent loop, the providers, the policies, the five stores
  tauri.conf.json   frameless window; frontendDist points at ../ui
ui/                 the pages -- plain HTML/CSS/JS, no build step
  shell/            the window shell; the only IPC exit in the window
  shared/           theme.css, components.css, icons.js, schemes.js, host.js
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

`ui/vendor/strings.json` is the one string table -- 341 entries, two languages.
The pages read it because they have no host to ask, and Rust reads the same file
with `include_str!`. Edit it directly; there is no generator upstream of it any
more.

## Licence

MIT. See `LICENSE`.
