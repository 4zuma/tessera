<script lang="ts">
  import { tick } from 'svelte';
  import { app, type MenuRow } from '../lib/app.svelte';
  import Icon from './Icon.svelte';

  let el: HTMLDivElement | undefined = $state();
  let pos = $state({ x: 0, y: 0, ox: 'left', oy: 'top' });

  $effect(() => {
    const m = app.menu;
    if (!m) return;
    void tick().then(() => {
      if (!el) return;
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      const flipX = m.x + w > window.innerWidth - 8;
      const flipY = m.y + h > window.innerHeight - 8;
      pos = { x: flipX ? Math.max(8, m.x - w) : m.x, y: flipY ? Math.max(8, m.y - h) : m.y, ox: flipX ? 'right' : 'left', oy: flipY ? 'bottom' : 'top' };
      el.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus({ preventScroll: true });
    });
  });

  function run(r: MenuRow) {
    if (r.type !== 'item' || r.disabled) return;
    app.menu = null;
    r.run();
  }

  function onKey(e: KeyboardEvent) {
    if (!el) return;
    const items = [...el.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      e.stopPropagation();
      const n = e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
      items[n]?.focus();
    } else if (e.key === 'Tab') {
      e.preventDefault();
    }
  }

  function onDocDown(e: PointerEvent) {
    if (app.menu && el && !el.contains(e.target as Node)) app.menu = null;
  }
</script>

<svelte:document onpointerdown={onDocDown} />
<svelte:window onblur={() => (app.menu = null)} onresize={() => (app.menu = null)} />

{#if app.menu}
  <div
    class="menu acrylic"
    role="menu"
    tabindex="-1"
    bind:this={el}
    style:left="{pos.x}px"
    style:top="{pos.y}px"
    style:transform-origin="{pos.ox} {pos.oy}"
    onkeydown={onKey}
    oncontextmenu={e => e.preventDefault()}
  >
    {#each app.menu.rows as r, i (i)}
      {#if r.type === 'head'}
        <div class="mh caption">{r.label}</div>
      {:else if r.type === 'sep'}
        <div class="ms" role="separator"></div>
      {:else if r.type === 'note'}
        <div class="mn caption">{#if r.lock}<Icon name="lock" size={13} />{/if}<span>{r.label}</span></div>
      {:else}
        <button
          class="mi"
          class:danger={r.danger}
          role="menuitem"
          disabled={r.disabled}
          onclick={() => run(r)}
          onpointerenter={() => r.hint?.()}
          onfocus={() => r.hint?.()}
        >
          <Icon name={r.icon} />
          <span class="ml">{r.label}</span>
          {#if r.key}<kbd class="caption">{r.key}</kbd>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 50;
    min-width: 240px;
    max-width: 340px;
    padding: 4px;
    border-radius: var(--r-card);
    animation: menu-in var(--t-norm) var(--ease-decel);
  }
  @keyframes menu-in {
    from {
      opacity: 0;
      transform: translateY(-6px);
    }
  }
  .mh {
    padding: 6px 12px 4px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mi {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    height: 32px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--r-ctl);
    background: none;
    text-align: left;
  }
  .mi:hover:not(:disabled),
  .mi:focus-visible {
    background: var(--subtle-hover);
    outline: none;
  }
  .mi:active:not(:disabled) {
    background: var(--subtle-press);
  }
  .mi:disabled {
    color: var(--text-dis);
  }
  .mi.danger:not(:disabled) {
    color: var(--critical);
  }
  .ml {
    flex: 1;
    white-space: nowrap;
  }
  kbd {
    color: var(--text-3);
  }
  .ms {
    height: 1px;
    margin: 4px -4px;
    background: var(--divider);
  }
  .mn {
    display: flex;
    gap: 6px;
    align-items: flex-start;
    padding: 4px 12px 6px;
    color: var(--text-3);
  }
  .mn :global(svg) {
    margin-top: 2px;
  }
</style>
