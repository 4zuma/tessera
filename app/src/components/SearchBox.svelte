<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { fmtBytes } from '../lib/format';
  import { t, tn } from '../lib/i18n.svelte';
  import Icon from './Icon.svelte';

  /**
   * The search box (Ctrl+F), a WinUI AutoSuggestBox without suggestions: it lives in the
   * title bar on Windows 11 and in the command bar where Windows draws the title bar. What
   * it finds stays bright on the map; everything else dims. The count and total size of the
   * matches sit at its right end, or why the query can't be read.
   */
  let input: HTMLInputElement | undefined = $state();

  $effect(() => {
    if (app.searchFocus && input) {
      input.focus();
      input.select();
    }
  });

  const s = $derived(app.search);
  const on = $derived(s.text.trim() !== '');
  const summary = $derived.by(() => {
    if (!on) return '';
    if (s.error) return s.error;
    const r = s.result;
    if (!r) return '';
    return r.count ? t('search.found', { items: tn('items', r.count), size: fmtBytes(r.bytes) }) : t('search.none');
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (s.text) app.clearSearch();
      else {
        input?.blur();
        app.focusMap();
      }
    } else if (e.key === 'Enter') {
      e.preventDefault();
      app.searchNow();
    }
  }
</script>

<div class="search" class:on class:err={on && !!s.error} class:busy={s.busy} role="search">
  <span class="glyph"><Icon name="search" size={14} /></span>
  <input
    bind:this={input}
    type="text"
    spellcheck="false"
    autocomplete="off"
    placeholder={t('search.placeholder', { drive: app.volume?.root.slice(0, 2) ?? '' })}
    title={t('search.tip')}
    aria-label={t('search.label')}
    aria-describedby="search-summary"
    value={s.text}
    oninput={e => app.setSearch(e.currentTarget.value)}
    onkeydown={onKey}
  />
  <span id="search-summary" class="sum caption tabular" aria-live="polite" title={summary}>{summary}</span>
  {#if s.text}
    <button class="x" onclick={() => { app.clearSearch(); input?.focus(); }} aria-label={t('search.clear')} title={t('search.clear')}>
      <Icon name="dismiss" size={12} />
    </button>
  {/if}
</div>

<style>
  /* WinUI TextBox: control fill, a stronger bottom stroke, the accent underline when focused */
  .search {
    position: relative;
    display: flex;
    align-items: center;
    height: 32px;
    min-width: 0;
    border-radius: var(--r-ctl);
    border: 1px solid var(--ctl-stroke);
    border-bottom-color: var(--ctl-strong);
    background: var(--ctl);
    transition: background-color var(--t-fast) linear;
    -webkit-app-region: no-drag;
  }
  .search:hover {
    background: var(--ctl-hover);
  }
  .search:focus-within {
    background: var(--ctl-input-active);
    border-bottom-color: transparent;
  }
  .search:focus-within::after {
    content: '';
    position: absolute;
    left: -1px;
    right: -1px;
    bottom: -1px;
    height: 2px;
    border-radius: 0 0 var(--r-ctl) var(--r-ctl);
    background: var(--accent);
  }
  .glyph {
    display: grid;
    place-items: center;
    width: 32px;
    flex: none;
    color: var(--text-2);
  }
  input {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0 4px 0 0;
    border: 0;
    background: transparent;
    user-select: text;
    -webkit-user-select: text;
  }
  input::placeholder {
    color: var(--text-2);
  }
  input:focus-visible {
    outline: none;
  }
  .sum {
    flex: none;
    max-width: 55%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0 4px;
    color: var(--text-2);
    transition: opacity var(--t-fast) linear;
  }
  .sum:empty {
    display: none;
  }
  .busy .sum {
    opacity: 0.6;
  }
  .err .sum {
    color: var(--critical);
  }
  .x {
    display: grid;
    place-items: center;
    flex: none;
    width: 28px;
    height: 24px;
    margin-right: 2px;
    padding: 0;
    border: 0;
    border-radius: var(--r-ctl);
    background: transparent;
    color: var(--text-2);
  }
  .x:hover {
    background: var(--subtle-hover);
  }
  .x:active {
    background: var(--subtle-press);
  }
</style>
