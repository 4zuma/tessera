<script lang="ts">
  import { onMount } from 'svelte';
  import { app } from './lib/app.svelte';
  import TitleBar from './components/TitleBar.svelte';
  import DrivePicker from './components/DrivePicker.svelte';
  import CommandBar from './components/CommandBar.svelte';
  import Stage from './components/Stage.svelte';
  import Pane from './components/Pane.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import ContextMenu from './components/ContextMenu.svelte';
  import ConfirmDialog from './components/ConfirmDialog.svelte';
  import Toast from './components/Toast.svelte';
  import Tooltip from './components/Tooltip.svelte';
  import { FLUENT } from './lib/treemap/ease';
  import { t } from './lib/i18n.svelte';

  let width = $state(1200);
  let paneWanted = $state(true);
  const paneOpen = $derived(paneWanted && width >= 820);
  const browser = $derived(!app.platform?.native);

  function togglePane() {
    app.paneToggled();
    paneWanted = !paneWanted;
  }

  /**
   * The details pane slides in from the window edge, clipped to its own column, as WinUI's
   * SplitView does with an inline pane: 200 ms in, 100 ms out, KeySpline 0,0.35 0.15,1.
   * The stage takes its new width once and the treemap animates into it.
   */
  function paneSlide(node: HTMLElement, { duration }: { duration: number }) {
    const w = node.offsetWidth;
    return {
      duration: app.reduced ? 0 : duration,
      easing: FLUENT.pane,
      css: (t: number) => {
        const d = ((1 - t) * w).toFixed(2);
        return `transform: translateX(${d}px); clip-path: inset(0 ${d}px 0 0);`;
      },
    };
  }

  onMount(() => {
    void app.init().catch(err => {
      console.error(err);
      app.error = String(err);
    });
  });
</script>

<svelte:window bind:innerWidth={width} onkeydown={e => app.onKey(e)} />

<div class="window" class:framed={browser && width >= 900}>
  {#if browser}<TitleBar />{/if}
  {#if app.error}
    <div class="fatal" role="alert">
      <h1>{t('app.startFailed')}</h1>
      <p>{app.error}</p>
    </div>
  {:else if app.phase === 'boot'}
    <div class="boot" aria-busy="true"></div>
  {:else if app.phase === 'pick'}
    <DrivePicker />
  {:else}
    <CommandBar {paneOpen} canTogglePane={width >= 820} onTogglePane={togglePane} />
    <div class="main">
      <Stage />
      {#if paneOpen}
        <div class="pane-col" in:paneSlide={{ duration: 200 }} out:paneSlide={{ duration: 100 }}><Pane /></div>
      {/if}
    </div>
    <StatusBar />
  {/if}
  <ContextMenu />
  <ConfirmDialog />
  <Toast />
  <Tooltip />
</div>

<style>
  .window {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow: hidden;
  }
  /* browser preview only: draw the app as a window on a desk */
  .window.framed {
    height: calc(100% - 40px);
    margin: 20px;
    border-radius: 8px;
    border: 1px solid var(--ctl-stroke-2);
    background: linear-gradient(135deg, var(--mica-a), var(--mica-b));
    box-shadow: var(--shadow-dialog);
  }
  :global(html[data-host='browser']) :global(body) {
    background: var(--desk, var(--solid-base));
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
    gap: 8px;
    padding: 0 8px 8px;
  }
  .pane-col {
    flex: none;
    display: flex;
    min-height: 0;
  }
  .boot {
    flex: 1;
  }
  .fatal {
    margin: auto;
    max-width: 520px;
    padding: 24px;
    text-align: center;
  }
  .fatal h1 {
    font: 600 20px/28px var(--font-display);
    margin: 0 0 8px;
  }
  .fatal p {
    color: var(--text-2);
    overflow-wrap: anywhere;
    user-select: text;
  }
</style>
