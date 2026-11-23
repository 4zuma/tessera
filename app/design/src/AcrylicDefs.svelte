<script lang="ts">
  // The acrylic blend (ui.css › .acrylic) needs the #acrylic filter that App.svelte draws. A
  // preview that shows a menu, toast or tooltip without the whole window gets it from here.
  import { app } from '../../src/lib/app.svelte';
</script>

<svg style="position: absolute; width: 0; height: 0" aria-hidden="true" focusable="false">
  <filter id="acrylic" x="0" y="0" width="1" height="1" color-interpolation-filters="sRGB">
    <feColorMatrix type="matrix" values={app.theme.acrylic.matrix} result="blend" />
    <feFlood flood-color={app.theme.acrylic.base} result="base" />
    <feComposite in="blend" in2="base" operator="over" />
  </filter>
</svg>
