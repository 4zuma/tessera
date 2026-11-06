<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { fmtBytes, fmtPct } from '../lib/format';
  import { t as tr } from '../lib/i18n.svelte';

  let w = $state(0);
  let h = $state(0);
  const pos = $derived.by(() => {
    const t = app.tip;
    if (!t) return { x: 0, y: 0 };
    let x = t.x + 16;
    let y = t.y + 20;
    if (x + w > window.innerWidth - 8) x = t.x - w - 12;
    if (y + h > window.innerHeight - 8) y = t.y - h - 12;
    return { x: Math.max(4, x), y: Math.max(4, y) };
  });
</script>

{#if app.tip}
  {@const t = app.tip}
  <div class="tip acrylic caption" bind:offsetWidth={w} bind:offsetHeight={h} style:transform="translate({Math.round(pos.x)}px, {Math.round(pos.y)}px)" aria-hidden="true">
    <div class="n">{t.name || '…'}</div>
    <div class="m tabular"><b>{fmtBytes(t.size)}</b><span>{t.kind}{t.share != null ? ` · ${tr('shareParent', { pct: fmtPct(t.share) })}` : ''}</span></div>
  </div>
{/if}

<style>
  .tip {
    position: fixed;
    left: 0;
    top: 0;
    z-index: 40;
    max-width: 320px;
    padding: 6px 10px 7px;
    border-radius: var(--r-ctl);
    pointer-events: none;
  }
  .n {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .m {
    display: flex;
    gap: 8px;
    white-space: nowrap;
  }
  .m span {
    color: var(--text-2);
  }
</style>
