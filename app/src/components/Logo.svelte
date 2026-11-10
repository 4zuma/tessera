<script lang="ts">
  // The 16 px artwork of the app icon (docs/brand/tessera-16.svg, drawn by scripts/brand-icons.mjs).
  // Keep the two in step; the colours are fixed so the logo reads the same on any accent colour.
  let { size = 16 }: { size?: number } = $props();
  const uid = $props.id();
  const clip = `logo-${uid}`;
</script>

<svg viewBox="0 0 16 16" width={size} height={size} aria-hidden="true" class="logo">
  <defs><clipPath id={clip}><circle cx="8" cy="8" r="6" /></clipPath></defs>
  <circle cx="8" cy="8" r="7.5" fill="#F4F7FB" stroke="#14213A" />
  <g clip-path="url(#{clip})">
    <rect x="0" y="0" width="10" height="16" fill="#2A7FE6" />
    <rect x="11" y="0" width="5" height="6" fill="#6CBBF7" />
    <rect x="11" y="7" width="5" height="9" fill="#1B1F5E" />
  </g>
  <circle cx="8" cy="8" r="1.3" fill="#F4F7FB" />
</svg>

<style>
  .logo {
    flex: none;
  }
</style>
