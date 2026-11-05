<script lang="ts">
  let { size = 16 }: { size?: number } = $props();
</script>

<svg viewBox="0 0 24 24" width={size} height={size} aria-hidden="true" class="logo">
  <rect x="2" y="2" width="11" height="12" rx="2" />
  <rect x="14.5" y="2" width="7.5" height="7" rx="2" opacity="0.75" />
  <rect x="14.5" y="10.5" width="7.5" height="11.5" rx="2" opacity="0.9" />
  <rect x="2" y="15.5" width="11" height="6.5" rx="2" opacity="0.6" />
</svg>

<style>
  .logo {
    flex: none;
    fill: var(--accent);
  }
</style>
