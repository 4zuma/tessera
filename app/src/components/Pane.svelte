<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { CAT_NAMES } from '../lib/backend/categories';
  import { Flag } from '../lib/backend/codec';
  import { fmtBytes, fmtDate, fmtInt, fmtPct } from '../lib/format';
  import Icon from './Icon.svelte';

  const d = $derived(app.details);
  const kindLabel = $derived.by(() => {
    if (!d) return '';
    if (d.kind === 'dir') return d.id === app.view ? '当前文件夹' : '文件夹';
    if (d.kind === 'file') return CAT_NAMES[d.cat] ?? '文件';
    if (d.kind === 'free') return '可用空间';
    if (d.kind === 'unscanned') return '扫描中';
    return '合并显示';
  });
  const legendTotal = $derived(app.legend.reduce((a, c) => a + c.bytes, 0) || 1);
</script>

<aside class="pane">
  <section class="card details" aria-live="polite">
    {#if d}
      <div class="dh">
        <span class="dic" style:background={d.kind === 'file' && d.cat < 12 ? `var(--cat-${d.cat})` : 'var(--ctl)'}>
          <Icon name={d.kind === 'dir' ? 'folder' : d.kind === 'file' ? (d.flags & Flag.Cloud ? 'cloud' : 'file') : d.kind === 'free' ? 'drive' : 'more'} size={18} />
        </span>
        <div class="dt">
          <div class="dn" title={d.name}>{d.name}</div>
          <div class="dk caption">{kindLabel}</div>
        </div>
      </div>
      <div class="dsize tabular">{fmtBytes(d.size)}</div>
      {#if d.kind !== 'group' && !(d.id === app.crumbs[0]?.id && app.sizeMode === 'logical')}
        <div class="share">
          <div class="bar"><i style:width="{Math.min(100, d.share * 100).toFixed(1)}%"></i></div>
          <span class="caption tabular">{d.kind === 'free' || d.kind === 'unscanned' || d.id === app.crumbs[0]?.id ? '占整个盘' : '占上级'} {fmtPct(d.share)}</span>
        </div>
      {/if}
      {#if d.kind === 'file' || d.kind === 'dir'}
        <dl class="kv caption tabular">
          <dt class:on={app.sizeMode === 'disk'}>占用空间</dt>
          <dd>{fmtBytes(d.disk)}</dd>
          <dt class:on={app.sizeMode === 'logical'}>实际大小</dt>
          <dd>{fmtBytes(d.logical)}</dd>
          {#if d.kind === 'dir'}
            <dt>包含</dt>
            <dd>{fmtInt(d.files)} 个文件{d.dirs ? ` · ${fmtInt(d.dirs)} 个文件夹` : ''}</dd>
          {:else}
            <dt>修改时间</dt>
            <dd>{fmtDate(d.modified)}</dd>
          {/if}
        </dl>
        <div class="path caption">{d.path}</div>
      {/if}
      {#if d.lockReason && d.id !== app.view && (d.kind === 'file' || d.kind === 'dir')}
        <div class="note warn caption"><Icon name="lock" size={14} /><span>{d.lockReason}</span></div>
      {/if}
      {#if d.hint}
        <div class="note caption"><Icon name="info" size={14} /><span>{d.hint}</span></div>
      {/if}
    {:else}
      <div class="empty caption">把鼠标移到方块上查看详情</div>
    {/if}
  </section>

  <section class="card sel">
    {#if app.selInfo.count}
      <div class="st">已选 <b>{app.selInfo.count}</b> 项 · <b class="tabular">{fmtBytes(app.selInfo.bytes)}</b></div>
      <div class="sa">
        <button class="btn" disabled={!!app.selInfo.lockReason} onclick={() => app.recycle([...app.sel])}><Icon name="trash" />移到回收站</button>
        <button class="btn danger" disabled={!!app.selInfo.lockReason} onclick={() => app.confirmPurge([...app.sel])}><Icon name="purge" />永久删除</button>
      </div>
      {#if app.selInfo.lockReason}
        <div class="why caption"><Icon name="lock" size={14} /><span>{app.selInfo.lockReason}</span></div>
      {/if}
    {:else}
      <div class="empty caption">单击选择 · Ctrl+单击多选 · 双击放大 · 右键查看更多操作</div>
    {/if}
  </section>

  <section class="card legend">
    <h2>按类型</h2>
    {#if app.legend.length}
      <div class="stack" aria-hidden="true">
        {#each app.legend as c (c.cat)}
          <i style:flex-grow={c.bytes / legendTotal} style:background="var(--cat-{c.cat})" class:dim={app.filter != null && app.filter !== c.cat}></i>
        {/each}
      </div>
      <div class="rows">
        {#each app.legend as c (c.cat)}
          <button class="lg" class:on={app.filter === c.cat} aria-pressed={app.filter === c.cat} onclick={() => app.setFilter(c.cat)} title="只突出显示{CAT_NAMES[c.cat]}">
            <i class="sw" style:background="var(--cat-{c.cat})"></i>
            <span class="ln">{CAT_NAMES[c.cat]}</span>
            <span class="lv caption tabular">{fmtBytes(c.bytes)}</span>
          </button>
        {/each}
      </div>
    {:else}
      <div class="empty caption">扫描后显示</div>
    {/if}
  </section>
</aside>

<style>
  .pane {
    width: 300px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow: auto;
    scrollbar-width: thin;
  }
  .card {
    padding: 14px 16px;
  }
  .empty {
    color: var(--text-3);
  }
  .dh {
    display: flex;
    gap: 12px;
    align-items: center;
    min-width: 0;
  }
  .dic {
    flex: none;
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    border-radius: 6px;
    border: 1px solid var(--ctl-stroke);
  }
  .dt {
    min-width: 0;
  }
  .dn {
    font-weight: 600;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .dk {
    color: var(--text-2);
  }
  .dsize {
    font: 600 28px/36px var(--font-display);
    margin: 12px 0 6px;
  }
  .share {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-2);
  }
  .bar {
    flex: 1;
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
    transition: width var(--t-slow) var(--ease-decel);
  }
  .kv {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    margin: 12px 0 8px;
  }
  .kv dt {
    color: var(--text-2);
  }
  .kv dt.on {
    color: var(--accent);
    font-weight: 600;
  }
  .kv dd {
    margin: 0;
    text-align: right;
  }
  .path {
    color: var(--text-3);
    overflow-wrap: anywhere;
    user-select: text;
    -webkit-user-select: text;
  }
  .note {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin-top: 10px;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--accent-soft);
  }
  .note :global(svg) {
    margin-top: 1px;
  }
  .note.warn {
    background: var(--critical-bg);
  }
  .st {
    margin-bottom: 10px;
  }
  .sa {
    display: flex;
    gap: 8px;
  }
  .sa .btn {
    flex: 1;
    padding: 0 8px;
  }
  .why {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 8px;
    color: var(--text-2);
  }
  h2 {
    font: 600 14px/20px var(--font);
    margin: 0 0 10px;
  }
  .stack {
    display: flex;
    height: 8px;
    gap: 2px;
    border-radius: 4px;
    overflow: hidden;
    margin-bottom: 8px;
  }
  .stack i {
    flex-basis: 0;
    min-width: 2px;
    transition: opacity var(--t-fast) linear;
  }
  .stack i.dim {
    opacity: 0.25;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin: 0 -6px;
  }
  .lg {
    display: grid;
    grid-template-columns: 12px 1fr auto;
    gap: 10px;
    align-items: center;
    height: 30px;
    padding: 0 6px;
    border: 0;
    border-radius: var(--r-ctl);
    background: none;
    text-align: left;
  }
  .lg:hover {
    background: var(--subtle-hover);
  }
  .lg.on {
    background: var(--accent-soft);
  }
  .sw {
    width: 12px;
    height: 12px;
    border-radius: 3px;
  }
  .lv {
    color: var(--text-2);
  }
</style>
