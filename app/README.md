# Tessera app (UI + Tauri shell)

The window and everything you see. The file tree itself lives in the Rust scan core;
this side only receives the rectangles that are on screen (see [`docs/ipc.md`](../docs/ipc.md)).

- `src/lib/backend/` – the IPC contract in TypeScript: the binary layout codec, the Tauri
  backend, and a mock backend with a realistic sample C: and D: (used until the core is
  wired in, and for the browser preview).
- `src/lib/treemap/` – the canvas engine: one tweened item per rect id, camera zoom,
  hit-testing, and a frame loop that stops when nothing moves (0 fps when idle).
- `src/components/` – Fluent (Windows 11) UI in Svelte 5.
- `src-tauri/` – the Tauri 2 crate: Mica backdrop, system accent colour, elevation.

## Run

```sh
npm install
npm run dev          # browser preview with the mock disk, http://localhost:1420
npm test             # codec + mock backend tests
npm run check        # svelte-check / TypeScript
npx tauri dev        # the real window (Windows; needs Rust and WebView2)
npx tauri build      # src-tauri/target/release/tessera-app.exe
```

Keyboard: arrows move the selection, Enter zooms in, Backspace goes up, Delete moves to
the recycle bin, Shift+Delete deletes permanently, Ctrl+Z undoes, Ctrl+C copies paths,
F5 rescans, Ctrl+Shift+F shows the frame-rate overlay.
