Acrylic: the material of everything that floats (menus, flyouts, tooltips, toasts). The backdrop blurred by 30 px, blended with `acrylic` through the app's colour matrix, 2% noise, a `ctl-stroke` border and `shadow`; `acrylic-solid` when transparency effects are off.

It needs something behind it to blur: put it over content. `padding` defaults to 4, `radius` to 8. In markup it is `<div class="acrylic">` (the app also draws the `#acrylic` SVG filter it uses; the bundle adds it when there is no `Window` on the page).
