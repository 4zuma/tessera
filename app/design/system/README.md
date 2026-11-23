Tessera is a disk-space map for Windows: one window, a treemap of a drive, a details pane, and nothing that is not about finding and freeing space. It follows Windows 11 (WinUI 3 / Fluent) so it feels like part of the system, and it is built from the app's own code: the colours are the app's theme, the components are the app's components running on sample data. Change the app, then sync this system; never restyle a component here.

## Content fundamentals

- **Voice.** Plain, calm, specific. Say what happened and what it costs: "Moved to the Recycle Bin: docker_data.vhdx (41.5 GB)". No exclamation marks, no jokes, no emoji.
- **Person.** Address the user as "you" only when needed ("You can undo this"); the app never says "I" or "we".
- **Case.** Sentence case for everything: buttons ("Move to Recycle Bin", "Delete permanently"), menu items ("Show in File Explorer"), titles ("Delete permanently?"). Product names keep theirs ("File Explorer", "Recycle Bin").
- **Numbers.** Sizes always with a unit and one decimal from GB down: "41.5 GB", "849 KB". Shares as "47% of parent", "100% of drive". Counts with thousands separators: "25,646 files".
- **Languages.** The UI follows the Windows display language: English, or Simplified Chinese (简体中文). Every string comes from the dictionaries (`app/src/lib/locales`); never hard-code text in a component. Chinese uses full-width punctuation and no spaces around numbers with units ("已选 1 项 · 41.5 GB").
- **Consequences first.** Destructive actions name the item and the size, and say what cannot be undone: "This skips the Recycle Bin and can't be undone. It frees 41.5 GB right away."

## Visual foundations

**Surfaces.** The window sits on Mica (`mica-a` → `mica-b` stands in for it in a browser; Windows 10 and "Transparency effects off" get `solid-base`). Regions of the window are `card` fills with a `card-stroke` border and `r-card` corners. Transient surfaces (context menu, flyouts, tooltip, toast) are acrylic: the `.acrylic` class blurs the backdrop by 30 px, blends it with `acrylic` and adds 2% noise; `acrylic-solid` is its fallback. Dialogs use `dlg-top` over `solid-base`, a `dlg-stroke` border, `shadow-dialog`, and `smoke` dims the window behind them.

**Colour.** Neutral first. The only hue in the chrome is `accent` (the user's Windows accent; the tokens hold the default blue) for the one main action, the focus pill under a chosen segment, and selection. `critical` is only for permanent deletion (`btn danger`, the menu's "Delete permanently"). `caution` marks admin hints and locked items. Text is `text` on every surface, `text-2` for secondary facts (sizes, paths), `text-3` for hints, `text-dis` when disabled. Colour in the map comes from file categories: `cat-0`…`cat-11` fill tiles, `cat-N-ink` labels them. Coloured by age instead (the pane's switch), tiles run from `age-new` (red, changed this week) to `age-old` (blue, over three years): `age-0`…`age-4` with `age-N-ink`, blended in OKLCH so the middle stays vivid.

**Type.** Segoe UI Variable (Text for UI, Display at 20 px and up), falling back to Segoe UI and Microsoft YaHei UI for Chinese. The ramp is Fluent's: `type-caption` 12/16, `type-body` 14/20, `type-body-strong` 14/20 semibold, `type-subtitle` 20/28 and `type-title` 28/36 semibold in Display. The treemap draws its labels at 12/16 (`type-tile-label`, `type-tile-label-strong` for folder titles). Segoe UI Variable is Windows' own font and ships with Windows only: on other systems previews fall back to the next face in the stack.

**Spacing and size.** A 4 px grid: `space-1` 4, `space-2` 8 (between window regions), `space-3` 12 (button side padding), `space-4` 16 (card padding), `space-6` 24 (dialogs). Controls are `control-height` 32 px tall. The treemap has its own: `tm-outer` around the map, `tm-pad` inside open folders, `tm-gap` between tiles, `tm-header` for a folder's title band, and no label on a tile narrower than `tm-label-min-w` or lower than `tm-label-min-h`.

**Corners.** `r-ctl` 4 px for controls and menu rows, `r-card` 8 px for cards, menus, dialogs and the window; tiles `tm-radius` 3 px, open folders `tm-folder-radius` 4 px.

**Borders and elevation.** Controls have a `ctl-stroke` border with a darker `ctl-stroke-2` bottom edge (Fluent's elevation line); pressed controls drop it. Elevation is a shadow only on what floats: `shadow` for menus, tooltips and toasts, `shadow-dialog` for dialogs. Cards and tiles have no shadow.

**States.** Hover and press change only fill, stroke or brightness, never size, position, padding or radius: a hovered tile gets the `tm-hover` light, tile edges near the pointer catch the `tm-reveal` light, a selected tile a 2 px `accent` ring with a thin light inner line, a hovered button `ctl-hover`, a pressed one `ctl-press`. Every coloured button variant sets its own hover and pressed fill (`accent-hover`/`accent-press`, `critical-hover`/`critical-press`). Disabled is `text-dis` on the normal fill. Focus is a 2 px `text`-coloured ring outside the control, shown for keyboard focus only.

**Motion.** Fluent timing: `t-fast` 167 ms for hover fills, `t-norm` 250 ms for flyouts, `t-slow` 333 ms for dialogs; `ease-decel` for things coming in, `ease-accel` for things leaving, `ease-fast` for moves. The details pane slides like WinUI's SplitView (200 ms in, 100 ms out). The treemap animates every relayout at 60 fps and draws nothing when idle. Reduced motion turns all of it off.

## The treemap

The map is the app; everything else serves it. Rules the painter and the layout keep:

- Area is size. A tile never changes geometry on hover or selection.
- A folder opens (gets a title band and shows its children) only when it is wide enough for a title (`minHeaderW`), big enough to hold a few labelled tiles (`minArea`), and over 0.25% of the map. Otherwise it stays closed: a `tm-leafdir` tile tinted by what fills it most.
- A chain of single-child folders folds into one tile titled `a › b › c`.
- Siblings too small to see merge into one "N small items" tile; clicking it zooms in. No tile is thinner than `tm-min-side` either way, and no folder tile narrower than `tm-min-width`.
- Tiles too small for a label get none; the tooltip names them.
- Merged small items are dotted (`tm-dots`); locked items (system files, no access, in use) are hatched with `tm-hatch`, and a locked open folder shows a lock in its title band.
- Free space is a faint `accent` wash with a dashed `accent` border; space not scanned yet is striped with `tm-stripe` behind a dashed `ctl-strong` border, and the stripes move while the scan runs.

## Iconography

29 line icons in the spirit of Fluent System Icons: a 24-unit grid, 1.5 stroke, round caps and joins, `currentColor`, drawn at 16 px (12 px in breadcrumbs). Use `Icon` with a name from its list; never emoji, never a filled icon. Caption buttons (minimise, maximise, close) use Segoe Fluent Icons, as Windows does. The logo is the app icon's 16 px artwork (`Logo`, Logos group); its colours are fixed and do not follow the accent.

## Layout

One window: title bar (logo, name, search, caption buttons) · command bar (drive, up, breadcrumbs, Rescan/Stop, size toggle, pane toggle) · the map and the 300 px details pane side by side with `space-2` between · status bar. The pane hides below 820 px of window width. Overlays (menu, tooltip, toast, dialog) float above everything.

## Behaviour rules

- Recycling never asks: it acts and offers Undo in a toast. Permanent deletion and emptying the Recycle Bin always ask, in a centred dialog over a dimmed window, with the destructive button in `critical`.
- Selection is Explorer's: click selects, Ctrl toggles, Shift selects the run of siblings in size order, Ctrl+Shift adds the run.
- Right-click opens Tessera's menu; "Show more options" (Shift+F10, Shift+right-click) opens Windows' own.

## Using the components

`window.Tessera` holds React 18 components; the page must load React 18 first. They are the app's real Svelte components, so they show exactly what the app draws, on a built-in sample drive. One app runs per page: every app component (all but `Button`, `SubtleButton`, `Card`, `Acrylic`, `Icon`, `Logo`) shares it, and their page props (`scenario`, `scheme`, `lang`, treemap sizes) set that app, the last one set winning. Use `Window` for a whole screen and pick its state with `scenario`; put one `Window` on a page at most. To try a treemap look, give `Window` or `Treemap` the treemap props (`pad`, `gap`, `header`, `radius`…) and compare. For anything new, compose plain markup with the classes `btn`, `btn accent`, `btn danger`, `sbtn`, `card` and `acrylic` and the tokens above.

## Not synced

- **Fonts.** Segoe UI Variable and Segoe Fluent Icons are licensed with Windows and are not copied here; `type.fonts` is empty on purpose.
- **Icons** live in the component bundle (`Icon`), not as asset files: the app draws them from one SVG path table (`app/src/lib/icons.ts`).
- **What Windows draws, not the app:** Mica itself (a gradient stands in; `Window backdrop="solid"` shows the solid colour Windows uses without it), the window frame and shadow, and Windows' own context menu behind "Show more options".
- **Data** is the app's built-in sample drive (the mock backend), never a real disk.
