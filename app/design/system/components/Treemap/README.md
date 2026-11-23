The treemap on its own: the app's canvas/WebGL painter and its layout, on the sample drive.

Give it a sized slot (it fills it). Use it to try the map's look without the window around it: `pad` (inset inside open folders, app 3), `gap` (between tiles, app 2), `header` (folder title band, app 21), `minHeaderW` (60), `minArea` (110), `labelMinW` (44), `labelMinH` (20), `radius` (tiles, 3) and `folderRadius` (open folders, 4) all apply live. It also takes `scenario`, `scheme` and `lang`.

Its tiles are drawn on a canvas, so they cannot be selected or restyled one by one in an editor: change the props, or change `app/src/lib/treemap` in the app. Hover and selection never change a tile's geometry.
