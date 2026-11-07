<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { fmtBytes } from '../lib/format';
  import { t } from '../lib/i18n.svelte';
  import Icon from './Icon.svelte';

  let { paneOpen, canTogglePane, onTogglePane }: { paneOpen: boolean; canTogglePane: boolean; onTogglePane: () => void } = $props();

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

  <button class="sbtn" onclick={() => app.zoomOut()} disabled={app.up == null} title={t('cmd.upTip')} aria-label={t('cmd.up')}>
    <Icon name="up" />
  </button>

  <nav class="crumbs" aria-label={t('cmd.path')}>
    {#each app.crumbs as c, i (c.id)}
      {#if i > 0}<span class="sep" aria-hidden="true"><Icon name="chev" size={12} /></span>{/if}
      <button class="crumb" class:cur={i === app.crumbs.length - 1} onclick={() => app.zoomTo(c.id)} aria-current={i === app.crumbs.length - 1 ? 'location' : undefined}>
        {c.name}
      </button>
    {/each}
  </nav>

  <span class="sp"></span>

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
  .crumbs {
    display: flex;
    align-items: center;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    margin-left: 4px;
  }
  .crumb {
    flex: 0 1 auto;
    min-width: 24px;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border: 0;
    background: none;
    padding: 4px 6px;
    border-radius: var(--r-ctl);
    color: var(--text-2);
    font: 600 14px/20px var(--font);
  }
  .crumb:hover {
    background: var(--subtle-hover);
    color: var(--text);
  }
  .crumb.cur {
    flex-shrink: 0;
    max-width: 320px;
    color: var(--text);
  }
  .sep {
    flex: none;
    display: inline-flex;
    color: var(--text-3);
    margin: 0 1px;
  }
  .sp {
    flex: 1;
    min-width: 8px;
  }
  /*
   * Segmented control, as Windows 11 draws it (Community Toolkit "Segmented"): a sunken
   * track, the chosen segment raised like a button with a short accent pill under its label.
   * The raised segment covers the track's border, so no edge is drawn twice, and both labels
   * keep the primary text colour: the surface says which one is on, not the ink.
   */
  .seg {
    display: inline-flex;
    flex: none;
    height: 32px;
    /* with the bar's 4 px gap: 8 px to the neighbours on both sides */
    margin: 0 4px;
    border: 1px solid var(--ctl-stroke);
    border-radius: var(--r-ctl);
    background: var(--ctl-alt);
  }
  .seg button {
    position: relative;
    margin: -1px;
    padding: 0 12px;
    border: 1px solid transparent;
    border-radius: var(--r-ctl);
    background: transparent;
    color: var(--text);
    white-space: nowrap;
    transition: background var(--t-fast) linear, border-color var(--t-fast) linear;
  }
  .seg button:hover:not(.on) {
    background: var(--subtle-hover);
  }
  .seg button:active:not(.on) {
    background: var(--subtle-press);
    color: var(--text-2);
  }
  .seg button.on {
    z-index: 1;
    background: var(--ctl);
    border-color: var(--ctl-stroke);
    border-bottom-color: var(--ctl-stroke-2);
  }
  /* the pill grows from 4 to 16 px when a segment is chosen, and shrinks while it is pressed */
  .seg button::after {
    content: '';
    position: absolute;
    left: 50%;
    bottom: 1px;
    width: 16px;
    height: 3px;
    margin-left: -8px;
    border-radius: 1.5px;
    background: var(--accent);
    opacity: 0;
    transform: scaleX(0.25);
    transition: transform var(--t-fast) var(--ease-decel), opacity var(--t-fast) linear;
  }
  .seg button.on::after {
    opacity: 1;
    transform: none;
  }
  .seg button.on:active::after {
    transform: scaleX(0.5);
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
