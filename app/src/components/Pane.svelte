<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { Flag } from '../lib/backend/codec';
  import { AGE_EDGE_CHOICES, AGE_EDGES, AGE_UNKNOWN, bandTotals } from '../lib/age';
  import { bandName, fmtBytes, fmtDate, fmtPct, fmtSpan } from '../lib/format';
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
  const legendTotal = $derived(app.legend.reduce((a, c) => a + c.bytes, 0) || 1);

  // by age: one row per band, newest first, and the bytes whose date is not known (bin 0)
  const bands = $derived(bandTotals(app.ageHist, app.ageEdges).map((bytes, i) => ({ i, bytes })));
  const unknownBytes = $derived(app.ageHist[AGE_UNKNOWN] ?? 0);
  const bandsTotal = $derived(bands.reduce((a, b) => a + b.bytes, 0) + unknownBytes || 1);
  const MAX_EDGES = 6;
  let editing = $state(false);
  const isDefault = $derived(app.ageEdges.length === AGE_EDGES.length && app.ageEdges.every((e, i) => e === AGE_EDGES[i]));
  function toggleEdge(d: number) {
    const on = app.ageEdges.includes(d);
    if (on && app.ageEdges.length === 1) return;
    if (!on && app.ageEdges.length >= MAX_EDGES) return;
    app.setAgeEdges(on ? app.ageEdges.filter(e => e !== d) : [...app.ageEdges, d]);
  }
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

  <!-- The buttons stay put with nothing selected, greyed out, so the card says what it is for
       before anything is picked; the two hint lines say how to pick. -->
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

  <section class="card legend">
    <div class="seg" role="radiogroup" aria-label={t('pane.colorBy')}>
      <button role="radio" aria-checked={app.colorBy === 'type'} class:on={app.colorBy === 'type'} onclick={() => app.setColorBy('type')} title={t('pane.typeTip')}>{t('pane.byType')}</button>
      <button role="radio" aria-checked={app.colorBy === 'age'} class:on={app.colorBy === 'age'} onclick={() => app.setColorBy('age')} title={t('pane.ageTip')}>{t('pane.byAge')}</button>
    </div>
    {#if app.colorBy === 'age'}
      {#if app.ageHist.length}
        <div class="stack" aria-hidden="true">
          {#each bands as b (b.i)}
            {#if b.bytes > 0}
              <i style:flex-grow={b.bytes / bandsTotal} style:background={app.coloring.fill[b.i]} class:dim={app.filter != null && app.filter !== b.i}></i>
            {/if}
          {/each}
          {#if unknownBytes > 0}
            <i style:flex-grow={unknownBytes / bandsTotal} class="unk" class:dim={app.filter != null}></i>
          {/if}
        </div>
        <div class="rows">
          {#each bands as b (b.i)}
            {@const name = bandName(b.i, app.ageEdges)}
            <button class="lg" class:on={app.filter === b.i} class:zero={!b.bytes} aria-pressed={app.filter === b.i} onclick={() => app.setFilter(b.i)} title={t('pane.highlight', { cat: name })}>
              <i class="sw" style:background={app.coloring.fill[b.i]}></i>
              <span class="ln">{name}</span>
              <span class="lv caption tabular">{fmtBytes(b.bytes)}</span>
            </button>
          {/each}
          {#if unknownBytes > 0}
            <div class="lg static">
              <i class="sw unk"></i>
              <span class="ln">{bandName(-1, app.ageEdges)}</span>
              <span class="lv caption tabular">{fmtBytes(unknownBytes)}</span>
            </div>
          {/if}
        </div>
      {:else}
        <div class="empty caption">{t('pane.afterScan')}</div>
      {/if}
      {#if editing}
        <div class="edges">
          <div class="caption hint">{t('pane.bandsHint')}</div>
          <div class="chips">
            {#each AGE_EDGE_CHOICES as d (d)}
              {@const on = app.ageEdges.includes(d)}
              <button class="chip caption" class:on aria-pressed={on} disabled={on ? app.ageEdges.length === 1 : app.ageEdges.length >= MAX_EDGES} onclick={() => toggleEdge(d)}>{fmtSpan(d)}</button>
            {/each}
          </div>
          <div class="ea">
            <button class="btn" disabled={isDefault} onclick={() => app.setAgeEdges(AGE_EDGES)}>{t('pane.bandsReset')}</button>
            <button class="btn accent" onclick={() => (editing = false)}>{t('pane.bandsDone')}</button>
          </div>
        </div>
      {:else}
        <button class="link caption" onclick={() => (editing = true)}><Icon name="edit" size={14} />{t('pane.editBands')}</button>
      {/if}
    {:else if app.legend.length}
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
  .legend .seg {
    display: flex;
    margin-bottom: 12px;
  }
  .legend .seg button {
    flex: 1;
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
  .lg.zero .ln,
  .lg.zero .lv {
    color: var(--text-3);
  }
  .lg.static {
    cursor: default;
  }
  .lg.static:hover {
    background: none;
  }
  /* the date is not known: no colour, as such tiles are drawn */
  .unk {
    background: var(--tm-leafdir);
    box-shadow: inset 0 0 0 1px var(--tm-leafdir-stroke);
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin: 8px -6px 0;
    padding: 4px 6px;
    border: 0;
    border-radius: var(--r-ctl);
    background: none;
    color: var(--accent);
  }
  .link:hover {
    background: var(--subtle-hover);
  }
  .edges {
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--divider);
  }
  .hint {
    color: var(--text-2);
    margin-bottom: 8px;
  }
  .chips {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
  }
  .chip {
    height: 28px;
    padding: 0 6px;
    border: 1px solid var(--ctl-stroke);
    border-radius: var(--r-ctl);
    background: var(--ctl);
    color: var(--text);
    white-space: nowrap;
  }
  .chip:hover:not(:disabled) {
    background: var(--ctl-hover);
  }
  .chip.on {
    background: var(--accent);
    border-color: transparent;
    color: var(--on-accent);
  }
  .chip.on:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .chip:disabled:not(.on) {
    color: var(--text-dis);
  }
  .ea {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 10px;
  }
</style>
