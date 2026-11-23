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
| `project/design-system.json` | the index for the first publish only; `npm run design:sync` replaces it with the published one for a re-sync |
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

To add a prop: page props go into `PageProps` in `src/runtime.ts` (treemap ones also into
`applyTreemapStyle`) and `PAGE_KEYS` in `src/wrap.ts` (a word, not a number, also into
`WORD_KEYS`, or it arrives as `NaN`); a prop of the window frame only goes through `attrs` in
`src/index.ts`, as `frame` and `backdrop` do. Document it in `index.d.ts` and the component's README.

`system/components/index.d.ts` lists every component and prop. To add a component: wrap it in
`src/index.ts`, add it to `COMPONENTS` in `build.mjs`, its types to `index.d.ts`, and a
`README.md` and `preview.html` under `system/components/<Name>/` (line 1 of a preview:
`<!-- @dsCard group="…" height=N width=N -->`).

## Publish

Publishing is done by Claude with its Artifact tool (Claude Code or a project thread), not by a
script: both artifacts are private to their owner. Their links are in the project memory
(`claude-design`): the Design System "Tessera Fluent" and the Design canvas "Tessera 原型".

## Re-sync (every thread, after its own UI change)

Any thread whose change alters what the app draws or what this system documents (anything under
`app/src` or `app/design`) re-syncs both artifacts itself, once its branch is pushed. Nobody else
does it for you. Several threads may do this on the same day, so the steps below never overwrite
someone else's published work:

- **The build must contain everything already published.** The published index names the
  commits it was built from (`lastChange.via`); `design:sync` refuses a build that leaves one
  out, so work from a branch that has not merged yet stays on the canvas until it merges.
- **The Artifact tool refuses to overwrite a path you have not read**, or one that changed since
  you read it. On a refusal someone published in between: read again, run `design:sync` again
  (it will usually ask you to merge their commit), publish again. Never pass `force`.

Steps:

1. **Build** from your pushed branch: `git fetch origin`, then `cd app && npm run design`.
2. **Read** the published system: Artifact `list` with `scope: "files"` on the system's link,
   then one `read` with `paths` = every `project/…` path it lists (not `SKILL.md`, `index.html`
   or `artifact-type/…`, which belong to the type) and `out_dir` = a new, empty scratch folder.
3. **Check**: `npm run design:sync -- --live <that folder> --note "<what changed, in a line>"`.
   - "this build leaves out <sha> (<branch>)": make a throwaway local branch,
     `git checkout -B ds-sync <your branch> && git merge origin/<that branch>`, go back to step 1
     on it (keep your own branch as it is; never push `ds-sync`). If that branch's PR was closed
     without merging, pass `--drop <sha>` instead and say so in the note.
   - "Nothing to publish": stop here; the canvas needs nothing either.
   - Otherwise it prints the `files` map to publish and writes the index to publish last
     (`design/out/project/design-system.json`: the published index with only `lastChange` new).
4. **Publish the files**: one Artifact publish with `url` = the system, `root` = `app/design/out`,
   `file_path` = one of the listed paths, `files` = the rest of the printed map.
5. **Publish the index** last: same `url` and `root`, `file_path` =
   `app/design/out/project/design-system.json`. Keep the version id this publish returns.
6. **Re-copy into the canvas** (below), then tell the user, with the canvas link, what to look at.

### The canvas after a re-sync

The canvas keeps its own copy of the system under `project/ds/tessera/` (`tokens.json`,
`tokens.css`, `components/bundle.js`, `bundle.css`) and a `designSystems` record in
`project/canvas.json`.

1. `read` the canvas's `project/canvas.json`, each `project/ds/tessera/…` file step 3 said to
   copy, and every artboard you will change, into a new scratch folder.
2. In that `canvas.json` set `designSystems[0].version` to the version id from step 5 and
   `copiedAt` to now. Change nothing else in it unless you add an artboard.
3. Publish with `url` = the canvas, `root` = that folder, `file_path` = its `project/canvas.json`,
   `files` = the copies step 3 printed
   (`"project/ds/tessera/components/bundle.js": {"artifact": "<system link>", "path": "project/components/bundle.js"}`,
   copied server side) plus any artboard you changed.

## The canvas

"Tessera 原型" holds one artboard per state, each a `Tessera.Window` or `Tessera.Treemap`
mounted with `<x-import component-from-global-scope="Tessera.Window" scenario="menu" …>` after
loading `ds/tessera/tokens.css`, `components/bundle.css` and `components/bundle.js`. Its
Tweaks are the page props above (`scenario`, `scheme`, `lang`, `colorBy`, the treemap sizes,
`backdrop`); markup props are kebab-case (`folder-radius`, `label-min-w`). Each artboard is its
own page, so each runs its own app. Text, layout and colours in the artboards can be edited by
hand on the canvas; what a component draws changes only in the app's code.

Adding or changing artboards:

- Edit the copy you just read, never an older local copy or one written from scratch: the user
  edits artboards by hand. Change only your own artboards; never move, rename or delete others'.
- A new artboard copies an existing one's markup: `support.js`, the three `ds/tessera/` links,
  the `x-import` with `{{holes}}`, `data-props` (each Tweak with a `section`) and the defaults in
  `renderVals()`. One app runs per page, so put each variant on its own artboard.
- Place it in free space (read the `x`, `y`, `w`, `h` of the boards in `canvas.json`; keep 80 px
  between boards), add it to `boards` and `order`, and give a new group a `title1` note above it.
- Publish only `canvas.json` (when changed) and the artboards you changed.

## First publish (once; done 2026-11-23)

1. Create one artifact from the Design System type, titled "Tessera Fluent", with no files.
2. Upload `assets/Logos/tessera.svg` and `tessera-16.svg` to it as assets (`asset: true`).
3. Put their records in `design-system.json` under `assetGroups.Logos.files`
   (`{"name", "blob", "size", "type"}`, `blob` = the id from `/_blob/<id>`) and in `order`;
   set `lastChange.by` to the owner's name.
4. Publish everything else under `project/` in one call (`root` = `design/out`,
   `file_path` = the index, `files` = every other path except the two SVGs).
   `components/index.d.ts` needs `"contentType": "text/plain"` (`.ts` is not a served type).
   After that, never send the index `build.mjs` writes: the published one holds the asset
   records and `createdOnFiles` (`design:sync` writes the right one).
