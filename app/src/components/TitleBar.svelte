<script lang="ts">
  import { onMount } from 'svelte';
  import { app } from '../lib/app.svelte';
  import Icon from './Icon.svelte';
  import Logo from './Logo.svelte';
  import SearchBox from './SearchBox.svelte';
  import { t } from '../lib/i18n.svelte';

  /**
   * The title bar. On Windows 11 the app draws it itself, as WinUI apps do (see
   * app/src-tauri/src/titlebar.rs): `app-region: drag` makes it a real caption for Windows
   * (drag, double-click, Aero Snap, the system menu), and the caption buttons are ours. In
   * the browser preview it is a picture of a window with a few preview controls. The search
   * box sits in its middle once a drive is open, as in WinUI apps' title bars.
   */
  const native = $derived(!!app.platform?.native);

  let active = $state(true);
  let bar: HTMLElement | undefined = $state();
  let maxBtn: HTMLButtonElement | undefined = $state();

  /** where the maximize button is, for the snap layouts flyout (Rust watches that spot) */
  let sent = '';
  function report() {
    if (!native || !maxBtn) return;
    const r = maxBtn.getBoundingClientRect();
    const right = document.documentElement.clientWidth - r.right;
    const rect = { right, top: r.top, width: r.width, height: r.height };
    const key = JSON.stringify(rect);
    if (key === sent) return;
    sent = key;
    void app.backend?.captionButton(rect);
  }

  onMount(() => {
    active = document.hasFocus();
  });
  // The buttons come once the platform is known, and the page can still be in the preview's
  // frame then; whenever the bar changes size, the button may have moved, so say so again.
  $effect(() => {
    if (!maxBtn || !native || !bar) return;
    report();
    const watch = new ResizeObserver(report);
    watch.observe(bar);
    return () => watch.disconnect();
  });
</script>

<svelte:window onfocus={() => (active = true)} onblur={() => (active = false)} />

<header class="tb" class:native class:inactive={!active} bind:this={bar}>
  <div class="l">
    <Logo />
    <span class="t">Tessera</span>
    {#if !native}
      <span class="chip caption" title={t('preview.chipTip')}>{t('preview.chip')}</span>
    {/if}
  </div>
  <div class="mid">
    {#if app.phase === 'map'}<SearchBox />{/if}
  </div>
  <div class="r">
    {#if !native}
      {#if app.phase === 'map' && app.volume?.root === 'C:\\'}
        <button class="sbtn caption" onclick={() => app.demoDownload()} disabled={app.scanning} title={t('preview.downloadTip')}>
          <Icon name="download" size={14} />{t('preview.download')}
        </button>
      {/if}
      <button class="sbtn caption" onclick={() => app.toggleHud()} aria-pressed={app.showHud} title={t('preview.perfTip')}>{t('preview.perf')}</button>
      <div class="wc" aria-hidden="true"><span class="min"></span><span class="max"></span><span class="close"></span></div>
    {:else}
      <!-- Windows' own caption buttons are not in the tab order either: Alt+Space opens their menu -->
      <div class="cap">
        <button tabindex="-1" onclick={() => app.backend?.caption('min')} aria-label={t('caption.min')} title={t('caption.min')}>
          <span class="g" aria-hidden="true">&#xE921;</span>
        </button>
        <button
          tabindex="-1"
          bind:this={maxBtn}
          class:hover={app.caption.hover}
          class:pressed={app.caption.pressed}
          onclick={() => app.backend?.caption('max')}
          aria-label={app.caption.maximized ? t('caption.restore') : t('caption.max')}
        >
          <span class="g" aria-hidden="true">{app.caption.maximized ? '\uE923' : '\uE922'}</span>
        </button>
        <button tabindex="-1" class="close" onclick={() => app.backend?.caption('close')} aria-label={t('close')} title={t('close')}>
          <span class="g" aria-hidden="true">&#xE8BB;</span>
        </button>
      </div>
    {/if}
  </div>
</header>

<style>
  /* three columns: the middle one stays centred while both sides fit, and the caption buttons never shrink */
  .tb {
    flex: none;
    height: 40px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(120px, 400px) minmax(max-content, 1fr);
    align-items: center;
    column-gap: 12px;
    padding-left: 14px;
  }
  .l,
  .r {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    align-self: stretch;
  }
  .l {
    overflow: hidden;
    white-space: nowrap;
  }
  .r {
    justify-content: flex-end;
  }
  /* Windows treats the bar as the window's caption; controls in it stay clickable */
  .tb.native {
    -webkit-app-region: drag;
    padding-left: 16px;
  }
  .tb.native button {
    -webkit-app-region: no-drag;
  }
  .t {
    font-size: 12px;
  }
  /* an inactive window dims its title and caption glyphs, as Windows does */
  .inactive .t,
  .inactive .cap .g {
    color: var(--text-3);
  }
  .inactive :global(.logo) {
    opacity: 0.6;
  }
  .chip {
    padding: 1px 8px;
    border-radius: 10px;
    background: var(--accent-soft);
    color: var(--text-2);
  }
  .sbtn {
    height: 28px;
    color: var(--text-2);
  }
  .sbtn[aria-pressed='true'] {
    color: var(--accent);
  }

  /* WinUI caption buttons: 46 px wide, the bar's height, Segoe Fluent Icons glyphs at 10 px */
  .cap {
    display: flex;
    align-self: stretch;
  }
  .cap button {
    width: 46px;
    height: 100%;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 0;
    background: transparent;
    color: var(--text);
    transition: background-color var(--t-fast) linear;
  }
  .g {
    font: 10px/1 'Segoe Fluent Icons', 'Segoe MDL2 Assets';
  }
  .cap button:hover,
  .cap button.hover {
    background: var(--subtle-hover);
  }
  .cap button:active,
  .cap button.pressed {
    background: var(--subtle-press);
    color: var(--text-2);
  }
  .cap button.close:hover {
    background: #c42b1c;
    color: #fff;
  }
  .cap button.close:active {
    background: rgba(196, 43, 28, 0.9);
    color: rgba(255, 255, 255, 0.7);
  }
  .cap button.close:hover .g,
  .cap button.close:active .g {
    color: inherit;
  }

  /* the preview's picture of caption buttons */
  .wc {
    display: flex;
    align-self: stretch;
  }
  .wc span {
    width: 46px;
    position: relative;
  }
  .wc span::before,
  .wc span::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    border-color: currentColor;
  }
  .wc .min::before {
    width: 10px;
    border-top: 1px solid;
    transform: translate(-50%, 0);
  }
  .wc .max::before {
    width: 10px;
    height: 10px;
    border: 1px solid;
    border-radius: 2px;
    transform: translate(-50%, -50%);
  }
  .wc .close::before,
  .wc .close::after {
    width: 13px;
    border-top: 1px solid;
  }
  .wc .close::before {
    transform: translate(-50%, 0) rotate(45deg);
  }
  .wc .close::after {
    transform: translate(-50%, 0) rotate(-45deg);
  }
  @media (max-width: 640px) {
    .wc,
    .chip {
      display: none;
    }
  }
</style>
