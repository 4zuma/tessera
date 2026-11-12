<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { Flag } from '../lib/backend/codec';
  import { fmtBytes, fmtDate, fmtPct } from '../lib/format';
  import { catName, t, tn, tParts } from '../lib/i18n.svelte';
  import Icon from './Icon.svelte';

  const d = $derived(app.details);
  const kindLabel = $derived.by(() => {
    if (!d) return '';
    if (d.kind === 'dir') return d.id === app.view ? t('kind.currentFolder') : t('folder');
    if (d.kind === 'file') return catName(d.cat) ?? t('file');
    if (d.kind === 'free') return t('free');
    if (d.kind === 'unscanned') return t('scanning');
    return t('kind.groupShort');
  });
  let tab = $state<'info' | 'top'>('info');
  const topMax = $derived(app.top[0]?.size || 1);
  const legendTotal = $derived(app.legend.reduce((a, c) => a + c.bytes, 0) || 1);
</script>

<aside class="pane">
  <div class="seg tabs" role="tablist" aria-label={t('pane.views')}>
    <button role="tab" aria-selected={tab === 'info'} class:on={tab === 'info'} onclick={() => (tab = 'info')}>{t('pane.info')}</button>
    <button role="tab" aria-selected={tab === 'top'} class:on={tab === 'top'} onclick={() => (tab = 'top')}>{t('pane.top')}</button>
  </div>

  {#snippet selection()}
  <section class="card sel">
    {#if app.selInfo.count}
      <div class="st">
        {#each tParts('selected', { n: app.selInfo.count, size: fmtBytes(app.selInfo.bytes) }) as p, i (i)}{#if p.v}<b class:tabular={p.v === 'size'}>{p.text}</b>{:else}{p.text}{/if}{/each}
      </div>
    {/if}
    <div class="sa">
      <button class="btn" disabled={!app.selInfo.count || !!app.selInfo.lockReason} onclick={() => app.recycle([...app.sel])}><Icon name="trash" />{t('recycle')}</button>
      <button class="btn danger" disabled={!app.selInfo.count || !!app.selInfo.lockReason} onclick={() => app.confirmPurge([...app.sel])}><Icon name="purge" />{t('purge')}</button>
    </div>
    {#if !app.selInfo.count}
      <div class="tip caption">
        <span>{t('pane.howToSelect')}</span>
        <span>{t('pane.howToMore')}</span>
      </div>
    {:else if app.selInfo.lockReason}
      <div class="why caption"><Icon name="lock" size={14} /><span>{app.selInfo.lockReason}</span></div>
    {/if}
  </section>
  {/snippet}

  {#if tab === 'info'}
  <div class="scroll">
  <div class="inner">
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
          <span class="caption tabular">{t(d.kind === 'free' || d.kind === 'unscanned' || d.id === app.crumbs[0]?.id ? 'shareDrive' : 'shareParent', { pct: fmtPct(d.share) })}</span>
        </div>
      {/if}
      {#if d.kind === 'file' || d.kind === 'dir'}
        <dl class="kv caption tabular">
          <dt class:on={app.sizeMode === 'disk'}>{t('sizeOnDisk')}</dt>
          <dd>{fmtBytes(d.disk)}</dd>
          <dt class:on={app.sizeMode === 'logical'}>{t('size')}</dt>
          <dd>{fmtBytes(d.logical)}</dd>
          {#if d.kind === 'dir'}
            <dt>{t('pane.contains')}</dt>
            <dd>{tn('files', d.files)}{d.dirs ? ` · ${tn('folders', d.dirs)}` : ''}</dd>
          {:else}
            <dt>{t('pane.modified')}</dt>
            <dd>{fmtDate(d.modified)}</dd>
          {/if}
        </dl>
        <div class="path caption">{d.path}</div>
      {/if}
      {#if d.lockReason && d.id !== app.view && (d.kind === 'file' || d.kind === 'dir')}
        <!-- a fact about the item, not an error: neutral, and it says first what is locked -->
        <div class="note locked caption" role="note">
          <Icon name="lock" size={14} />
          <div class="nb">
            <b>{d.kind === 'dir' ? t('pane.folderLocked') : t('pane.fileLocked')}</b>
            <span>{d.lockReason}</span>
          </div>
        </div>
      {/if}
      {#if d.hint}
        <div class="note caption"><Icon name="info" size={14} /><span>{d.hint}</span></div>
      {/if}
    {:else}
      <div class="empty caption">{t('pane.empty')}</div>
    {/if}
  </section>

  <section class="card legend">
    <h2>{t('pane.byType')}</h2>
    {#if app.legend.length}
      <div class="stack" aria-hidden="true">
        {#each app.legend as c (c.cat)}
          <i style:flex-grow={c.bytes / legendTotal} style:background="var(--cat-{c.cat})" class:dim={app.filter != null && app.filter !== c.cat}></i>
        {/each}
      </div>
      <div class="rows">
        {#each app.legend as c (c.cat)}
          <button class="lg" class:on={app.filter === c.cat} aria-pressed={app.filter === c.cat} onclick={() => app.setFilter(c.cat)} title={t('pane.highlight', { cat: catName(c.cat) ?? '' })}>
            <i class="sw" style:background="var(--cat-{c.cat})"></i>
            <span class="ln">{catName(c.cat)}</span>
            <span class="lv caption tabular">{fmtBytes(c.bytes)}</span>
          </button>
        {/each}
      </div>
    {:else}
      <div class="empty caption">{t('pane.afterScan')}</div>
    {/if}
  </section>
  </div>
  </div>
  <!-- The buttons stay put with nothing selected, greyed out, so the card says what it is for
       before anything is picked. The actions come last in both tabs and stay in reach: the cards above scroll when the
       pane is too short, this one keeps its place under them -->
  {@render selection()}
  {:else}
  <!-- the list keeps its place whatever is selected: the actions sit under it, not above -->
  <section class="card toplist">
      {#if app.top.length}
        <!-- pointing at a row lights the tile that holds the file, a click flies there -->
        <div class="scope caption">{t('pane.topIn', { name: app.topName })}</div>
        <div class="rows" role="group" aria-label={t('pane.top')} onpointerleave={() => app.peek(null)}>
          {#each app.top as f (f.id)}
            <button
              class="tf"
              class:on={app.sel.has(f.id)}
              title={t('pane.topGo', { name: f.name })}
              onpointerenter={() => app.peek(f)}
              onfocus={() => app.peek(f)}
              onblur={() => app.peek(null)}
              onclick={() => app.goTo(f)}
            >
              <i class="sw" style:background="var(--cat-{f.cat})"></i>
              <span class="tn">{f.name}</span>
              <span class="lv caption tabular">{fmtBytes(f.size)}</span>
              <span class="td caption">{f.dir || app.topName}</span>
              <span class="tb" aria-hidden="true"><i style:width="{((f.size / topMax) * 100).toFixed(1)}%" style:background="var(--cat-{f.cat})"></i></span>
            </button>
          {/each}
        </div>
      {:else}
        <div class="empty caption">{t('pane.afterScan')}</div>
      {/if}
      </section>
    {@render selection()}
  {/if}
</aside>

<style>
  .pane {
    width: 300px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
  }
  .scroll {
    flex: 0 1 auto;
    min-height: 0;
    overflow: auto;
    scrollbar-width: thin;
  }
  .inner {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .sel {
    flex: none;
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
    flex: none;
    margin-top: 1px;
  }
  /* InfoBar, informational: the subtle attention fill with a card stroke, no severity colour */
  .note.locked {
    background: var(--card-2);
    border: 1px solid var(--card-stroke);
    color: var(--text-2);
  }
  .nb {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .nb b {
    font-weight: 600;
    color: var(--text);
  }
  .st {
    margin-bottom: 10px;
  }
  /* side by side at equal widths; stacked when the labels need more room (English) */
  .sa {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .sa .btn {
    flex: 1;
    min-width: max-content;
    padding: 0 8px;
  }
  .tip {
    display: flex;
    flex-direction: column;
    margin-top: 10px;
    color: var(--text-3);
  }
  .why {
    display: flex;
    gap: 6px;
    align-items: flex-start;
    margin-top: 8px;
    color: var(--text-2);
  }
  /* the reason can run to a few lines: the lock stays with the first */
  .why :global(svg) {
    flex: none;
    margin-top: 1px;
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
  .tabs {
    display: flex;
    flex: none;
  }
  h2 {
    font: 600 14px/20px var(--font);
    margin: 0 0 10px;
  }
  .toplist {
    flex: 1;
    min-height: 120px;
    overflow: auto;
    scrollbar-width: thin;
    padding: 8px 10px;
  }
  .scope {
    padding: 2px 6px 6px;
    color: var(--text-2);
  }
  .toplist .rows {
    margin: 0;
  }
  .tabs button {
    flex: 1;
  }
  /* two lines per file: name and size, then where it lies; a hairline bar compares the sizes */
  .tf {
    display: grid;
    grid-template-columns: 12px 1fr auto;
    gap: 0 10px;
    align-items: center;
    padding: 5px 6px 6px;
    border: 0;
    border-radius: var(--r-ctl);
    background: none;
    text-align: left;
    min-width: 0;
  }
  .tf:hover,
  .tf:focus-visible {
    background: var(--subtle-hover);
  }
  .tf.on {
    background: var(--accent-soft);
  }
  .tn,
  .td {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .td {
    grid-column: 2 / 4;
    color: var(--text-3);
    /* long paths lose their start, not the folder the file is in */
  }
  .tb {
    grid-column: 2 / 4;
    height: 2px;
    margin-top: 3px;
    border-radius: 1px;
    background: var(--ctl-stroke-2);
    overflow: hidden;
  }
  .tb i {
    display: block;
    height: 100%;
    border-radius: 1px;
  }
</style>
