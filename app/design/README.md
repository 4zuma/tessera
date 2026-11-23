# app/design: Tessera's design system for Claude Design

This folder turns the app's own code into **Tessera Fluent**, a Design System artifact in Claude
(Claude Design), so visual ideas can be tried on a canvas against the real components instead of
a hand-written prototype each time.

Nothing here is a copy. The tokens are read from the app's theme, the components are the app's
real Svelte components (wrapped as React 18 components, which is what Claude Design loads), and
they run on the mock backend's sample drive. **Change the app, then rebuild and re-sync; never
restyle a component in Claude Design.** The published system is marked as synced from code
(`"editing": "code"`), so its page is read-only.

## Build

```sh
cd app
npm run design
```

`design/build.mjs` writes, under `design/out/` (gitignored):

| output | what |
| --- | --- |
| `project/components/bundle.js`, `bundle.css` | one IIFE script that sets `window.Tessera` (`src/index.ts`), and its stylesheet |
| `project/tokens.json`, `tokens.css` | colours (light and dark, from `src/lib/theme.ts`), type, spacing, radii, shadows, motion (`src/ui.css`, `src/lib/treemap/motion.ts`, `paint.ts`) |
| `project/README.md`, `components/*`, `assets/Logos/README.md` | copied from `system/` (hand-written: the brand book, one README and preview per component, the cover) |
| `project/assets/Logos/*.svg` | copied from `docs/brand/` |
| `project/design-system.json` | the index, for a **first** publish only (see below) |
| `app/` | the whole app in a browser on the mock backend (a plain `vite build`) |

`npm run check` type-checks `design/src` along with the app.

## How the components work

- `src/wrap.ts` makes a React component that mounts a Svelte component into its own `div` and
  passes React props through a `$state` object, so prop changes update the Svelte side.
- One app runs per page (`app` in `src/lib/app.svelte.ts` is a singleton). Every app component shares
  it; its page props (`scenario`, `scheme`, `lang`, the treemap sizes) set that app.
- `src/runtime.ts` reaches each `scenario` (menu open, dialog, toast, search…) through the app's
  own methods on the sample drive. `src/backend.ts` stands in for `src/lib/backend/index.ts`
  (swapped in `vite.config.mjs`): always the mock, with scans that finish at once or stay frozen
  part way.
- The treemap props change `fluentLayout` (`src/lib/treemap/motion.ts`) and `paintStyle`
  (`src/lib/treemap/paint.ts`) at run time and relayout. The app itself never changes them.
- `Button`, `SubtleButton`, `Card`, `Acrylic` are plain markup with the app's classes;
  `Icon` and `Logo` are standalone. None of them start the app.

`system/components/index.d.ts` lists every component and prop. To add a component: wrap it in
`src/index.ts`, add it to `COMPONENTS` in `build.mjs`, its types to `index.d.ts`, and a
`README.md` and `preview.html` under `system/components/<Name>/` (line 1 of a preview:
`<!-- @dsCard group="…" height=N width=N -->`).

## Publish and re-sync

Publishing is done by Claude with its Artifact tool (Claude Code or a project thread), not by a
script: the artifact is private to its owner. The system's link is kept in the project memory.

**First publish** (once; done 2026-11-23):

1. Create one artifact from the Design System type, titled "Tessera Fluent", with no files.
2. Upload `assets/Logos/tessera.svg` and `tessera-16.svg` to it as assets (`asset: true`).
3. Put their records in `design-system.json` under `assetGroups.Logos.files`
   (`{"name", "blob", "size", "type"}`, `blob` = the id from `/_blob/<id>`) and in `order`;
   set `lastChange.by` to the owner's name.
4. Publish everything else under `project/` in one call (`root` = `design/out`,
   `file_path` = the index, `files` = every other path except the two SVGs).

**Re-sync** after the app changed (rebuild first):

1. `read` the published `project/design-system.json`. Never send the one `build.mjs` wrote: the
   published one holds the asset records and `createdOnFiles`.
2. Publish the changed files (usually `components/bundle.js`, `bundle.css`, `tokens.json`,
   `tokens.css`, and any README or preview you touched) with `root` = `design/out`.
3. Last, publish the index you read in step 1 with only `lastChange` changed
   (`at` now, `via` "GitHub · 4zuma/tessera@<sha>", a `note`), keeping `"editing": "code"`.
4. The canvas "Tessera 原型" carries its own copy of the system under `project/ds/tessera/`
   (`tokens.json`, `tokens.css`, `components/bundle.js`, `bundle.css`): copy the new files there
   too, or its artboards keep the old components.
