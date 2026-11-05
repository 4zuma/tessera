<script lang="ts">
  import { app } from '../lib/app.svelte';
  import Icon from './Icon.svelte';
  import Logo from './Logo.svelte';
</script>

<!-- Browser preview only. In the app, Windows draws the real title bar over Mica. -->
<header class="tb">
  <Logo />
  <span class="t">Tessera</span>
  <span class="chip caption" title="这里是浏览器预览，用的是模拟的磁盘数据；在 Tessera 程序里会扫描真实的磁盘">预览 · 模拟数据</span>
  <span class="sp"></span>
  {#if app.phase === 'map' && app.volume?.root === 'C:\\'}
    <button class="sbtn caption" onclick={() => app.demoDownload()} disabled={app.scanning} title="模拟一个正在增长的下载文件，看实时更新时树图怎么动">
      <Icon name="download" size={14} />模拟下载
    </button>
  {/if}
  <button class="sbtn caption" onclick={() => app.toggleHud()} aria-pressed={app.showHud} title="显示帧率和矩形数 (Ctrl+Shift+F)">性能</button>
  <div class="wc" aria-hidden="true"><span class="min"></span><span class="max"></span><span class="close"></span></div>
</header>

<style>
  .tb {
    flex: none;
    height: 40px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding-left: 14px;
  }
  .t {
    font-size: 12px;
  }
  .chip {
    padding: 1px 8px;
    border-radius: 10px;
    background: var(--accent-soft);
    color: var(--text-2);
  }
  .sp {
    flex: 1;
  }
  .sbtn {
    height: 28px;
    color: var(--text-2);
  }
  .sbtn[aria-pressed='true'] {
    color: var(--accent);
  }
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
  .min::before {
    width: 10px;
    border-top: 1px solid;
    transform: translate(-50%, 0);
  }
  .max::before {
    width: 10px;
    height: 10px;
    border: 1px solid;
    border-radius: 2px;
    transform: translate(-50%, -50%);
  }
  .close::before,
  .close::after {
    width: 13px;
    border-top: 1px solid;
  }
  .close::before {
    transform: translate(-50%, 0) rotate(45deg);
  }
  .close::after {
    transform: translate(-50%, 0) rotate(-45deg);
  }
  @media (max-width: 640px) {
    .wc,
    .chip {
      display: none;
    }
  }
</style>
