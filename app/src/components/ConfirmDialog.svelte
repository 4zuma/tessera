<script lang="ts">
  /**
   * Confirmation for actions that cannot be undone (permanent delete, emptying the
   * recycle bin), shaped like a WinUI ContentDialog: centred in the window over a smoke
   * layer that blocks the app, the "do it" button on the left and Cancel on the right.
   * No default button, so Enter only presses the focused one, and focus starts on Cancel.
   * Clicking the smoke does not dismiss it; Esc does (handled in app.onKey).
   */
  import { tick } from 'svelte';
  import { app } from '../lib/app.svelte';
  import { t } from '../lib/i18n.svelte';

  let el: HTMLDivElement | undefined = $state();

  $effect(() => {
    if (!app.confirm) return;
    const prev = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    void tick().then(() => el?.querySelector<HTMLButtonElement>('.no')?.focus({ preventScroll: true }));
    return () => {
      if (prev?.isConnected && prev !== document.body) prev.focus({ preventScroll: true });
      else app.focusMap();
    };
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Tab' && el) {
      // keep focus inside the dialog
      const bs = [...el.querySelectorAll<HTMLButtonElement>('button')];
      const i = bs.indexOf(document.activeElement as HTMLButtonElement);
      e.preventDefault();
      bs[(i + (e.shiftKey ? bs.length - 1 : 1)) % bs.length]?.focus();
    }
  }
</script>

{#if app.confirm}
  {@const c = app.confirm}
  <div class="smoke">
    <div
      class="dlg"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="cd-title"
      aria-describedby="cd-body"
      tabindex="-1"
      bind:this={el}
      onkeydown={onKey}
    >
      <div class="top">
        <h2 id="cd-title">{c.title}</h2>
        <div class="lead"><span>{c.lead}</span>{#if c.size}<b class="tabular">{c.size}</b>{/if}</div>
        <p id="cd-body">{c.body}</p>
      </div>
      <div class="acts">
        <button class="btn" onclick={() => app.closeConfirm(true)}>{c.ok}</button>
        <button class="btn no" onclick={() => app.closeConfirm(false)}>{t('cancel')}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .smoke {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    padding: 16px;
    background: var(--smoke);
    animation: fade var(--t-fast) linear;
  }
  @keyframes fade {
    from {
      background: transparent;
    }
  }
  .dlg {
    width: 100%;
    min-width: 0;
    max-width: 448px;
    max-height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border: 1px solid var(--dlg-stroke);
    border-radius: var(--r-card);
    background: var(--solid-base);
    box-shadow: var(--shadow-dialog);
    animation: pop var(--t-norm) var(--ease-decel);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(1.05);
    }
  }
  .top {
    padding: 24px;
    background: var(--dlg-top);
    border-bottom: 1px solid var(--card-stroke);
    overflow: auto;
  }
  h2 {
    margin: 0 0 12px;
    font: 600 20px/28px var(--font-display);
  }
  .lead {
    display: flex;
    gap: 12px;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 4px;
    font-weight: 600;
  }
  .lead span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lead b {
    color: var(--critical);
    flex: none;
  }
  p {
    margin: 0;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  .acts {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    padding: 24px;
  }
</style>
