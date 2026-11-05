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
  import ConfirmFlyout from './components/ConfirmFlyout.svelte';
  import Toast from './components/Toast.svelte';
  import Tooltip from './components/Tooltip.svelte';

  let width = $state(1200);
  let paneWanted = $state(true);
  const paneOpen = $derived(paneWanted && width >= 820);
  const browser = $derived(!app.platform?.native);

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
      <h1>Tessera 没能启动</h1>
      <p>{app.error}</p>
    </div>
  {:else if app.phase === 'boot'}
    <div class="boot" aria-busy="true"></div>
  {:else if app.phase === 'pick'}
    <DrivePicker />
  {:else}
    <CommandBar {paneOpen} canTogglePane={width >= 820} onTogglePane={() => (paneWanted = !paneWanted)} />
    <div class="main">
      <Stage />
      {#if paneOpen}<Pane />{/if}
    </div>
    <StatusBar />
  {/if}
  <ContextMenu />
  <ConfirmFlyout />
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
