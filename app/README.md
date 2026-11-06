# Tessera app (UI + Tauri shell)

The window and everything you see. The file tree itself lives in the Rust scan core;
this side only receives the rectangles that are on screen (see [`docs/ipc.md`](../docs/ipc.md)).

- `src/lib/backend/` – the IPC contract in TypeScript: the binary layout codec, the Tauri
  backend, and a mock backend with a realistic sample C: and D: (for the browser preview
  and the tests).
- `src/lib/treemap/` – the canvas engine: one tweened item per rect id, camera zoom,
  hit-testing, and a frame loop that stops when nothing moves (0 fps when idle). Folders
  and tiles are drawn with WebGL (one instanced draw call) under a 2D canvas that holds
  the text, hover and selection; the settled map is cached, so hovering repaints only
  what changed. Without a hardware GPU everything is drawn on the 2D canvas
  (`localStorage['tessera.gpu']` = `off` forces that, `force` takes a software WebGL).
- `src/components/` – Fluent (Windows 11) UI in Svelte 5.
- `src-tauri/` – the Tauri 2 crate: the commands and events of the contract on top of
  `tessera-core` (`src/backend.rs`), the drive list, Mica backdrop, system accent colour,
  elevation. It is a member of the root Cargo workspace, so it builds into `../target`.

## Run

```sh
npm install
npm run dev          # browser preview with the mock disk, http://localhost:1420
npm test             # codec + mock backend tests
npm run check        # svelte-check / TypeScript
npx tauri dev        # the real window (Windows; needs Rust and WebView2)
npx tauri build      # ../target/release/tessera-app.exe
```

Keyboard: arrows move the selection, Enter zooms in, Backspace goes up, Delete moves to
the recycle bin, Shift+Delete deletes permanently, Ctrl+Z undoes, Ctrl+C copies paths,
F5 rescans, Ctrl+Shift+F shows the frame-rate overlay (with WebGL or 2D for the drawing path).

Mouse: click selects, Ctrl+click toggles a tile, Shift+click selects the run of siblings
between the last clicked tile and this one in layout (size) order, Ctrl+Shift+click adds
that run. Permanent delete and emptying the recycle bin ask in a centred dialog over a
dimmed window, as WinUI's ContentDialog does; moving to the recycle bin does not ask,
it can be undone.
