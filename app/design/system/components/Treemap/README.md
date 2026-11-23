The treemap on its own: the app's canvas/WebGL painter and its layout, on the sample drive.

Give it a sized slot (it fills it). Use it to try the map's look without the window around it: `pad` (inset inside open folders, app 3), `gap` (between tiles, app 2), `header` (folder title band, app 21), `minHeaderW` (60), `minArea` (110), `minSide` (24), `minWidth` (44), `labelMinW` (44), `labelMinH` (20), `radius` (tiles, 3) and `folderRadius` (open folders, 4) all apply live. It also takes `scenario`, `scheme`, `lang` and `colorBy` (`type`/`age`).

`folderLook="band"` draws the folder look being decided for 1.3.1 (not in the app yet): every folder is one frame, its title band's fill runs down into the white area's corners (and, with `ring`, on round the sides and bottom), content sits `white` px from the white area on all four sides, and a closed folder shows its colour as a `foot` px band along the bottom, `tone` steps lighter, fading upward over `fade` px. With it, `pad` and `header` follow from these and are ignored.

Its tiles are drawn on a canvas, so they cannot be selected or restyled one by one in an editor: change the props, or change `app/src/lib/treemap` in the app. Hover and selection never change a tile's geometry.
