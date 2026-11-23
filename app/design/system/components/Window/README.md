The whole Tessera window, exactly as the app draws it, on the built-in sample C: drive.

Use it for any full-screen design: one `Window` per page, sized by its slot (the app's layout rules apply: the details pane hides below 820 px of width). Pick what it shows with `scenario`:

| scenario | shows |
| --- | --- |
| `map` | the map, nothing selected (default) |
| `picker` | the drive picker before the first scan |
| `scanning` | a scan frozen part way: not-scanned stripes, Stop button |
| `selected` / `folder` | the biggest file / folder selected, its details in the pane |
| `menu` | Tessera's context menu on the biggest file |
| `confirm` | the "Delete permanently?" dialog |
| `search` | a search for `query` (default `*.vhdx`), everything else dimmed |
| `toast` | a file just recycled, with Undo |
| `tooltip` | the tooltip over a tile |

`scheme` (`light`/`dark`) and `lang` (`en`/`zh-CN`) pin the theme and language, and `colorBy` (`type`/`age`) colours the tiles by file type or by age; left out, the theme follows the page. The treemap props (`pad`, `gap`, `header`, `minHeaderW`, `minArea`, `minSide`, `minWidth`, `labelMinW`, `labelMinH`, `radius`, `folderRadius`) change the map live, for trying a look before changing the app. `frame="desk"` shows the browser preview's window on a desk instead of filling the slot. `backdrop="solid"` paints the window in the solid base colour (`--solid-base`) that Windows shows when Mica is off (Transparency effects off, battery saver) instead of the Mica-like wash; Windows 10 shows that colour too, but with Windows' own title bar, which the design does not draw.

It is live: click, right-click, zoom, search and delete all work on the sample data. Do not put a second app component that sets `scenario` on the same page.
