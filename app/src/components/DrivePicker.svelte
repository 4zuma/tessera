<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { fmtBytes } from '../lib/format';
  import Icon from './Icon.svelte';

  const sorted = $derived([...app.volumes].sort((a, b) => a.root.localeCompare(b.root)));
</script>

<section class="pick">
  <h1>选择要扫描的磁盘</h1>
  <p class="sub">扫描时树图会边扫边长出来，扫完之前也可以开始浏览。</p>
  <div class="drives">
    {#each sorted as v (v.root)}
      {@const used = v.capacity - v.free}
      {@const pct = v.capacity ? used / v.capacity : 0}
      <button class="drive card" onclick={() => app.startScan(v.root)}>
        <span class="ic"><Icon name="drive" size={28} /></span>
        <span class="info">
          <span class="name">{v.label} ({v.root.slice(0, 2)})</span>
          <span class="bar" class:low={pct > 0.9}><i style:width="{(pct * 100).toFixed(1)}%"></i></span>
          <span class="meta caption tabular">{fmtBytes(v.free)} 可用，共 {fmtBytes(v.capacity)} · {v.fs}</span>
        </span>
      </button>
    {/each}
  </div>
  {#if app.platform && !app.platform.elevated}
    <div class="hint card caption">
      <Icon name="shield" />
      <span>现在以普通权限运行，用多线程逐个目录扫描。以管理员身份运行时会直接读取 NTFS 的 MFT，通常快很多。</span>
      <button class="btn" onclick={() => app.relaunchElevated()}>以管理员身份重启</button>
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
  .hint {
    width: min(720px, 100%);
    margin-top: 16px;
    padding: 12px 14px;
    display: flex;
    align-items: center;
    gap: 12px;
    color: var(--text-2);
  }
  .hint span {
    flex: 1;
  }
  @media (max-width: 640px) {
    .pick {
      padding: 28px 16px;
    }
    .hint {
      flex-wrap: wrap;
    }
  }
</style>
