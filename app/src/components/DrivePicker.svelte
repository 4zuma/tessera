<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { fmtBytes } from '../lib/format';
  import { t } from '../lib/i18n.svelte';
  import Icon from './Icon.svelte';

  const sorted = $derived([...app.volumes].sort((a, b) => a.root.localeCompare(b.root)));
</script>

<section class="pick">
  <h1>{t('picker.title')}</h1>
  <p class="sub">{t('picker.sub')}</p>
  <div class="drives">
    {#each sorted as v (v.root)}
      {@const used = v.capacity - v.free}
      {@const pct = v.capacity ? used / v.capacity : 0}
      <button class="drive card" onclick={() => app.startScan(v.root)}>
        <span class="ic"><Icon name="drive" size={28} /></span>
        <span class="info">
          <span class="name">{v.label} ({v.root.slice(0, 2)})</span>
          <span class="bar" class:low={pct > 0.9}><i style:width="{(pct * 100).toFixed(1)}%"></i></span>
          <span class="meta caption tabular">{t('picker.space', { free: fmtBytes(v.free), capacity: fmtBytes(v.capacity), fs: v.fs })}</span>
        </span>
      </button>
    {/each}
  </div>
  {#if app.platform && !app.platform.elevated}
    <div class="hint" role="note">
      <span class="ti"><Icon name="info" /></span>
      <p class="msg"><strong>{t('picker.userTitle')}</strong>{t('picker.userBody')}</p>
      <button class="btn" onclick={() => app.relaunchElevated()}><Icon name="shield" />{t('restartAdmin')}</button>
    </div>
  {/if}
</section>

<style>
  .pick {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 48px 32px;
    display: flex;
    flex-direction: column;
    align-items: center;
  }
  h1 {
    font: 600 28px/36px var(--font-display);
    margin: 0;
  }
  .sub {
    margin: 6px 0 28px;
    color: var(--text-2);
  }
  .drives {
    width: min(720px, 100%);
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 8px;
  }
  .drive {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 16px 18px;
    text-align: left;
    transition: background var(--t-fast) linear;
  }
  .drive:hover {
    background: var(--ctl-hover);
  }
  .drive:active {
    background: var(--ctl-press);
  }
  .ic {
    color: var(--text-2);
  }
  .info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .name {
    font-weight: 600;
  }
  .bar {
    height: 4px;
    border-radius: 2px;
    background: var(--ctl-stroke-2);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    border-radius: 2px;
    background: var(--accent);
  }
  .bar.low i {
    background: var(--critical);
  }
  .meta {
    color: var(--text-2);
  }
  /* a Fluent InfoBar (informational): primary text, accent icon, standard button */
  .hint {
    width: min(720px, 100%);
    margin-top: 16px;
    padding: 12px 16px;
    display: flex;
    align-items: center;
    gap: 12px;
    background: var(--card-2);
    border: 1px solid var(--card-stroke);
    border-radius: var(--r-card);
  }
  .ti {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    flex: none;
    align-self: flex-start;
    color: var(--on-accent);
    background: var(--accent);
  }
  .ti :global(svg) {
    width: 13px;
    height: 13px;
    stroke-width: 2;
  }
  .msg {
    flex: 1;
    min-width: 0;
    margin: 0;
  }
  .msg strong {
    font-weight: 600;
    margin-right: 8px;
  }
  @media (max-width: 640px) {
    .pick {
      padding: 28px 16px;
    }
    .hint {
      flex-wrap: wrap;
    }
    .msg {
      flex-basis: calc(100% - 32px);
    }
    .hint .btn {
      margin-left: 32px;
    }
  }
</style>
