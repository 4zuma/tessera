<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { fmtBytes } from '../lib/format';
  import { t, tParts } from '../lib/i18n.svelte';

  const v = $derived(app.volume);
  const used = $derived(v ? v.capacity - v.free : 0);
  const low = $derived(!!v && v.free / v.capacity < 0.05);
</script>

<footer class="status caption tabular">
  {#if v}
    <span>{#each tParts('status.used', { used: fmtBytes(used), capacity: fmtBytes(v.capacity) }) as p, i (i)}{#if p.v === 'used'}<b>{p.text}</b>{:else}{p.text}{/if}{/each}</span>
    <span class="bar" class:low><i style:width="{((used / v.capacity) * 100).toFixed(2)}%"></i></span>
    <span>{#each tParts('status.free', { free: fmtBytes(v.free) }) as p, i (i)}{#if p.v}<b class:low>{p.text}</b>{:else}{p.text}{/if}{/each}</span>
    <span class="dot" aria-hidden="true"></span>
  {/if}
  <span class="scan" aria-live="polite">{app.scanText}</span>
  <span class="sp"></span>
  {#if app.bytesPerPx}
    <span class="scale" title={t('status.scaleTip')}>1 px² ≈ {fmtBytes(app.bytesPerPx)}</span>
  {/if}
</footer>

<style>
  .status {
    flex: none;
    height: 32px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 16px 2px;
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
  }
  b {
    color: var(--text);
    font-weight: 600;
  }
  b.low {
    color: var(--critical);
  }
  .bar {
    flex: none;
    width: 120px;
    height: 4px;
    border-radius: 2px;
    background: var(--ctl-stroke-2);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width var(--t-slow) var(--ease-decel);
  }
  .bar.low i {
    background: var(--critical);
  }
  .dot {
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: var(--text-3);
  }
  .scan {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sp {
    flex: 1;
  }
  .scale {
    color: var(--text-3);
  }
  @media (max-width: 760px) {
    .scale,
    .bar {
      display: none;
    }
  }
</style>
