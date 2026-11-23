<script lang="ts">
  import { app, type MenuRow } from '../lib/app.svelte';
  import { fmtBytes } from '../lib/format';
  import { groupName, t } from '../lib/i18n.svelte';
  import Icon from './Icon.svelte';
  import SearchBox from './SearchBox.svelte';

  /** one step of the path: a folder, or a level of small items inside the last folder */
  interface Step {
    key: string;
    name: string;
    level: boolean;
    go: () => void;
  }
  const steps = $derived<Step[]>([
    ...app.crumbs.map(c => ({ key: `n${c.id}`, name: c.name, level: false, go: () => void app.zoomTo(c.id) })),
    ...app.levels.map((l, i) => ({ key: `l${i}`, name: groupName(l.count, l.holds), level: true, go: () => app.backToLevel(i + 1) })),
  ]);

  // As in Explorer, a path too long for the bar folds from the left into a "…" step, which
  // takes the first folder's place: the drive stays first (C: › … › deeper › current), and
  // "…" lists the folded steps, nearest first. A hidden copy of every step measures them.
  let nav: HTMLElement | undefined = $state();
  let meas: HTMLElement | undefined = $state();
  /** how many steps at the end show after "…"; null: the whole path fits */
  let tail = $state<number | null>(null);
  const shown = $derived(tail == null || steps.length <= 2 ? steps : [steps[0], ...steps.slice(steps.length - tail)]);
  const folded = $derived(tail == null || steps.length <= 2 ? [] : steps.slice(1, steps.length - tail));

  function fit() {
    if (!nav || !meas) return;
    const avail = nav.clientWidth;
    const [sepEl, ellEl, ...stepEls] = [...meas.children] as HTMLElement[];
    // the separator's 1 px margins
    const sep = sepEl.offsetWidth + 2;
    const w = stepEls.map(el => el.offsetWidth);
    const n = w.length;
    const full = w.reduce((a, b) => a + b, 0) + sep * Math.max(0, n - 1);
    if (n <= 2 || full <= avail) {
      tail = null;
      return;
    }
    // the drive, "…" and as many of the last steps as fit; the current one always shows
    let used = w[0] + sep + ellEl.offsetWidth;
    let k = 0;
    for (let i = n - 1; i >= 1; i--) {
      if (k > 0 && used + sep + w[i] > avail) break;
      used += sep + w[i];
      k++;
    }
    tail = Math.min(k, n - 2);
  }

  $effect(() => {
    void steps;
    fit();
  });
  $effect(() => {
    if (!nav) return;
    const ro = new ResizeObserver(() => fit());
    ro.observe(nav);
    return () => ro.disconnect();
  });

  function openFolded(e: MouseEvent) {
    const b = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const rows: MenuRow[] = folded
      .slice()
      .reverse()
      .map(s => ({ type: 'item', icon: s.level ? 'tiles' : 'folder', label: s.name, run: s.go }));
    app.tip = null;
    app.menu = { x: b.left, y: b.bottom + 4, rows };
  }

  /** `search`: the search box is here, where Windows draws the title bar (Windows 10) */
  let {
    paneOpen,
    canTogglePane,
    onTogglePane,
    search,
  }: { paneOpen: boolean; canTogglePane: boolean; onTogglePane: () => void; search: boolean } = $props();

  let drivesOpen = $state(false);

  function pickDrive(root: string) {
    drivesOpen = false;
    void app.startScan(root);
  }
  function onDocDown(e: PointerEvent) {
    if (drivesOpen && !(e.target as HTMLElement).closest('.drives-fly, .drive-btn')) drivesOpen = false;
  }
</script>

<svelte:document onpointerdown={onDocDown} />

<div class="cmd">
  <div class="drive-wrap">
    <button
      class="sbtn drive-btn"
      aria-haspopup="menu"
      aria-expanded={drivesOpen}
      onclick={() => (drivesOpen = !drivesOpen)}
      title={t('cmd.chooseDrive')}
    >
      <Icon name="drive" />
      <span>{app.volume?.root.slice(0, 2) ?? t('cmd.drive')}</span>
      <Icon name="chevDown" size={12} />
    </button>
    {#if drivesOpen}
      <div class="drives-fly acrylic" role="menu">
        {#each app.volumes as v (v.root)}
          <button class="mi" role="menuitem" onclick={() => pickDrive(v.root)}>
            <Icon name="drive" />
            <span class="mi-l">{v.label} ({v.root.slice(0, 2)})</span>
            <span class="mi-k caption tabular">{t('cmd.driveFree', { size: fmtBytes(v.free) })}</span>
          </button>
        {/each}
        <div class="ms" role="separator"></div>
        <button class="mi" role="menuitem" onclick={() => { drivesOpen = false; app.phase = 'pick'; }}>
          <Icon name="more" /><span class="mi-l">{t('cmd.allDrives')}</span>
        </button>
      </div>
    {/if}
  </div>

  <button class="sbtn" onclick={() => app.zoomOut()} disabled={!app.canGoUp} title={t('cmd.upTip')} aria-label={t('cmd.up')}>
    <Icon name="up" />
  </button>

  <nav class="crumbs" aria-label={t('cmd.path')} bind:this={nav}>
    {#each shown as c, i (c.key)}
      {#if i > 0}<span class="sep" aria-hidden="true"><Icon name="chev" size={12} /></span>{/if}
      {#if i === 1 && folded.length}
        <button class="crumb ell" aria-haspopup="menu" onclick={openFolded} title={t('cmd.pathHidden')} aria-label={t('cmd.pathHidden')}>…</button>
        <span class="sep" aria-hidden="true"><Icon name="chev" size={12} /></span>
      {/if}
      {@const cur = i === shown.length - 1}
      <button class="crumb" class:cur class:lvl={c.level} onclick={c.go} aria-current={cur ? 'location' : undefined}>
        {#if c.level}<Icon name="tiles" size={14} />{/if}<span class="nm">{c.name}</span>
      </button>
    {/each}
    <!-- every step at its natural width, for fit() -->
    <div class="measure" aria-hidden="true" bind:this={meas}>
      <span class="sep"><Icon name="chev" size={12} /></span>
      <span class="crumb ell">…</span>
      {#each steps as c, i (c.key)}
        <span class="crumb" class:cur={i === steps.length - 1}>
          {#if c.level}<Icon name="tiles" size={14} />{/if}<span class="nm">{c.name}</span>
        </span>
      {/each}
    </div>
  </nav>

  {#if search}<div class="find"><SearchBox /></div>{/if}

  {#if app.scanning}
    <button class="btn" onclick={() => app.cancelScan()}><Icon name="stop" /><span class="lbl">{t('cmd.stop')}</span></button>
  {:else}
    <button class="btn" onclick={() => app.rescan()} title={t('cmd.rescanTip')}><Icon name="refresh" /><span class="lbl">{t('cmd.rescan')}</span></button>
  {/if}

  <div class="seg" role="radiogroup" aria-label={t('cmd.sizeMode')}>
    <button role="radio" aria-checked={app.sizeMode === 'disk'} class:on={app.sizeMode === 'disk'} onclick={() => app.setSizeMode('disk')} title={t('cmd.diskTip')}>{t('sizeOnDisk')}</button>
    <button role="radio" aria-checked={app.sizeMode === 'logical'} class:on={app.sizeMode === 'logical'} onclick={() => app.setSizeMode('logical')} title={t('cmd.logicalTip')}>{t('size')}</button>
  </div>

  {#if app.platform && !app.platform.elevated}
    <button class="sbtn admin" onclick={() => app.relaunchElevated()} title={t('cmd.adminTip')} aria-label={t('restartAdmin')}>
      <Icon name="shield" />
    </button>
  {/if}
  {#if canTogglePane}
    <button class="sbtn" onclick={onTogglePane} aria-pressed={paneOpen} title={paneOpen ? t('cmd.hidePane') : t('cmd.showPane')} aria-label={t('cmd.pane')}>
      <Icon name="pane" />
    </button>
  {/if}
</div>

<style>
  .cmd {
    flex: none;
    height: 48px;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 10px 0 8px;
    min-width: 0;
  }
  .drive-wrap {
    position: relative;
  }
  .drive-btn {
    gap: 6px;
    font-weight: 600;
  }
  .drives-fly {
    position: absolute;
    z-index: 30;
    top: 38px;
    left: 0;
    min-width: 260px;
    padding: 4px;
    border-radius: var(--r-card);
    animation: fly-in var(--t-norm) var(--ease-decel);
  }
  @keyframes fly-in {
    from {
      opacity: 0;
      transform: translateY(-8px);
    }
  }
  .mi {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    height: 34px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--r-ctl);
    background: none;
    text-align: left;
  }
  .mi:hover {
    background: var(--subtle-hover);
  }
  .mi-l {
    flex: 1;
    white-space: nowrap;
  }
  .mi-k {
    color: var(--text-3);
  }
  .ms {
    height: 1px;
    margin: 4px -4px;
    background: var(--divider);
  }
  /* the path takes the room the other controls leave; fit() folds it to that width */
  .crumbs {
    position: relative;
    flex: 1 1 0;
    display: flex;
    align-items: center;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    margin: 0 8px 0 4px;
  }
  .crumb {
    flex: 0 1 auto;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 24px;
    max-width: 220px;
    overflow: hidden;
    white-space: nowrap;
    border: 0;
    background: none;
    padding: 4px 6px;
    border-radius: var(--r-ctl);
    color: var(--text-2);
    font: 600 14px/20px var(--font);
  }
  .nm {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .crumb:hover {
    background: var(--subtle-hover);
    color: var(--text);
  }
  /* the steps before it fold away first (fit()); it gives way only when even
     "C: › … › current" does not fit */
  .crumb.cur {
    max-width: 320px;
    color: var(--text);
  }
  .ell {
    flex: none;
    min-width: 0;
    letter-spacing: 1px;
  }
  .sep {
    flex: none;
    display: inline-flex;
    color: var(--text-3);
    margin: 0 1px;
  }
  .measure {
    position: absolute;
    left: 0;
    top: 0;
    display: flex;
    width: max-content;
    visibility: hidden;
    pointer-events: none;
  }
  .measure .crumb {
    flex: none;
  }
  .find {
    flex: 0 1 260px;
    min-width: 140px;
    margin-right: 4px;
  }
  /* the segmented control (ui.css): with the bar's 4 px gap, 8 px to the neighbours on both sides */
  .seg {
    margin: 0 4px;
  }
  .sbtn[aria-pressed='true'] {
    color: var(--accent);
  }
  @media (max-width: 760px) {
    .lbl {
      display: none;
    }
  }
  @media (max-width: 520px) {
    .admin {
      display: none;
    }
    .seg button {
      padding: 0 8px;
    }
  }
</style>
