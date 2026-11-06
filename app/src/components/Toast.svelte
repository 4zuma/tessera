<script lang="ts">
  import { app } from '../lib/app.svelte';
  import type { IconName } from '../lib/icons';
  import Icon from './Icon.svelte';
  import { t as tr } from '../lib/i18n.svelte';

  const icons: Record<string, IconName> = { info: 'info', success: 'check', recycle: 'trash', error: 'warn' };
</script>

{#if app.toast}
  {@const t = app.toast}
  {#key t.id}
    <div class="toast acrylic {t.kind}" role="status" aria-live="polite">
      <span class="ti"><Icon name={icons[t.kind]} /></span>
      <div class="tm">
        {#if t.title}<div class="tt">{t.title}</div>{/if}
        <div class="tx">{t.text}</div>
      </div>
      {#if t.actions.length}
        <div class="ta">
          {#each t.actions as a, i (a.label)}
            <button class="btn" class:accent={i === 0} onclick={() => { app.hideToast(); a.run(); }}>
              {#if a.icon}<Icon name={a.icon} />{/if}{a.label}
            </button>
          {/each}
        </div>
      {/if}
      <button class="sbtn x" onclick={() => app.hideToast()} aria-label={tr('close')}><Icon name="dismiss" size={14} /></button>
    </div>
  {/key}
{/if}

<style>
  .toast {
    position: fixed;
    z-index: 70;
    left: 50%;
    bottom: 48px;
    transform: translateX(-50%);
    width: max-content;
    max-width: min(680px, calc(100vw - 32px));
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 8px 10px 16px;
    border-radius: var(--r-card);
    animation: rise var(--t-slow) var(--ease-decel);
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translate(-50%, 12px);
    }
  }
  .ti {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    flex: none;
    color: var(--on-accent);
    background: var(--accent);
  }
  .ti :global(svg) {
    width: 13px;
    height: 13px;
    stroke-width: 2;
  }
  .success .ti {
    background: #0f7b0f;
    color: #fff;
  }
  .error .ti {
    background: var(--critical);
    color: var(--on-critical);
  }
  .tm {
    min-width: 0;
    flex: 1;
  }
  .tt {
    font-weight: 600;
  }
  .tx {
    color: var(--text-2);
    white-space: pre-line;
    overflow-wrap: anywhere;
  }
  .ta {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .x {
    flex: none;
    color: var(--text-2);
  }
  @media (max-width: 640px) {
    .toast {
      flex-wrap: wrap;
    }
  }
</style>
