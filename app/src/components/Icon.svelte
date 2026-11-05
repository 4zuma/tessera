<script lang="ts">
  import { ICONS, type IconName } from '../lib/icons';
  let { name, size = 16 }: { name: IconName; size?: number } = $props();
</script>

<svg viewBox="0 0 24 24" width={size} height={size} aria-hidden="true" class="ico"><path d={ICONS[name]} /></svg>

<style>
  .ico {
    flex: none;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
</style>
