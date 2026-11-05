<script lang="ts">
  import { tick } from 'svelte';
  import { app } from '../lib/app.svelte';
  import Icon from './Icon.svelte';

  let el: HTMLDivElement | undefined = $state();
  let pos = $state<{ x: number; y: number } | null>(null);
  const anchored = $derived(!!app.confirm && app.confirm.anchor != null);

  $effect(() => {
    const c = app.confirm;
    if (!c) return;
    pos = null;
    void tick().then(() => {
      if (!el) return;
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      const box = c.anchor != null ? app.tileBox(c.anchor) : null;
      const sr = app.stageRect();
      if (box && sr) {
        // centred on the tile it is about, kept inside the window
        const cx = sr.left + box.x + box.w / 2;
        const cy = sr.top + box.y + box.h / 2;
        pos = {
          x: Math.min(Math.max(8, cx - w / 2), window.innerWidth - w - 8),
          y: Math.min(Math.max(8, cy - h / 2), window.innerHeight - h - 8),
        };
      } else {
        pos = { x: (window.innerWidth - w) / 2, y: (window.innerHeight - h) / 2 };
      }
      el.querySelector<HTMLButtonElement>('.no')?.focus({ preventScroll: true });
    });
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Tab' && el) {
      // keep focus inside the flyout
      const bs = [...el.querySelectorAll<HTMLButtonElement>('button')];
      const i = bs.indexOf(document.activeElement as HTMLButtonElement);
      e.preventDefault();
      bs[(i + (e.shiftKey ? bs.length - 1 : 1)) % bs.length]?.focus();
    }
  }
</script>

{#if app.confirm}
  {@const c = app.confirm}
  <div class="scrim" class:smoke={!anchored} onpointerdown={() => app.closeConfirm(false)} aria-hidden="true"></div>
  <div
    class="fly acrylic"
    class:dialog={!anchored}
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="cf-title"
    aria-describedby="cf-body"
    tabindex="-1"
    bind:this={el}
    style:left={pos ? `${pos.x}px` : '-9999px'}
    style:top={pos ? `${pos.y}px` : '0'}
    onkeydown={onKey}
  >
    <h2 id="cf-title">{c.title}</h2>
    <div class="lead"><span>{c.lead}</span>{#if c.size}<b class="tabular">{c.size}</b>{/if}</div>
    <p id="cf-body">{c.body}</p>
    <div class="acts">
      <button class="btn no" onclick={() => app.closeConfirm(false)}>取消</button>
      <button class="btn" class:danger={c.danger} class:accent={!c.danger} onclick={() => app.closeConfirm(true)}>
        {#if c.danger}<Icon name="purge" />{/if}{c.ok}
      </button>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
  }
  .scrim.smoke {
    background: var(--smoke);
    animation: fade var(--t-norm) linear;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  .fly {
    position: fixed;
    z-index: 61;
    width: 360px;
    max-width: calc(100vw - 16px);
    padding: 20px 20px 18px;
    border-radius: var(--r-card);
    animation: pop var(--t-slow) var(--ease-decel);
  }
  .fly.dialog {
    width: 420px;
    padding: 24px;
    background: var(--acrylic-solid);
    box-shadow: var(--shadow-dialog);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.96);
    }
  }
  h2 {
    margin: 0;
    font: 600 20px/28px var(--font-display);
  }
  .lead {
    display: flex;
    gap: 12px;
    justify-content: space-between;
    align-items: baseline;
    margin: 12px 0 6px;
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
    margin: 0 0 20px;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  .acts {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
</style>
