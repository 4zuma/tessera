<script lang="ts">
  import { onMount } from 'svelte';
  import { app } from '../lib/app.svelte';
  import { Kind } from '../lib/backend/codec';
  import { fmtInt } from '../lib/format';

  let stage: HTMLDivElement;
  let canvas: HTMLCanvasElement;

  onMount(() => {
    app.attach(canvas, stage);
    return () => app.detach();
  });

  function local(e: MouseEvent) {
    const r = canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function onMove(e: PointerEvent) {
    const eng = app.engine;
    if (!eng) return;
    const { x, y } = local(e);
    eng.mouse.x = x;
    eng.mouse.y = y;
    eng.mouse.in = true;
    eng.mouse.moved = true;
    if (app.menu || app.confirm) return eng.kick();
    const it = eng.hit(x, y);
    const id = it ? it.id : null;
    if (id !== app.hoverId || !app.tip) app.hover(id, e.clientX, e.clientY);
    else app.moveTip(e.clientX, e.clientY);
    canvas.style.cursor = it && it.r.kind === Kind.Dir && y - it.y < (it.r.header || 0) ? 'zoom-in' : 'default';
    eng.kick();
  }

  function onLeave() {
    const eng = app.engine;
    if (!eng) return;
    eng.mouse.in = false;
    app.hover(null);
    eng.kick();
  }

  function onDown(e: PointerEvent) {
    if (e.button === 3) {
      e.preventDefault();
      app.zoomOut();
      return;
    }
    if (e.button !== 0) return;
    const { x, y } = local(e);
    const it = app.engine?.hit(x, y) ?? null;
    app.click(it ? it.id : null, { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey });
  }

  function onDbl(e: MouseEvent) {
    const { x, y } = local(e);
    const it = app.engine?.hit(x, y);
    if (it) app.zoomInto(it.id);
  }

  function onMenu(e: MouseEvent) {
    e.preventDefault();
    const { x, y } = local(e);
    const it = app.engine?.hit(x, y) ?? null;
    app.openMenu(e.clientX, e.clientY, it ? it.id : null);
  }
</script>

<div class="stage" bind:this={stage}>
  <canvas
    bind:this={canvas}
    aria-label="磁盘占用矩形树图。方向键移动选择，回车放大，Backspace 返回上级，Delete 移到回收站"
    tabindex="0"
    onpointermove={onMove}
    onpointerleave={onLeave}
    onpointerdown={onDown}
    ondblclick={onDbl}
    oncontextmenu={onMenu}
    onmouseup={e => e.button === 3 && e.preventDefault()}
  ></canvas>
  {#if app.showHud}
    <div class="hud caption tabular" aria-hidden="true">
      <span>{fmtInt(app.stats.rects)} 个矩形</span>
      <span class:busy={app.stats.busy}>{app.stats.busy ? `${app.stats.fps} fps` : '空闲 · 0 fps'}</span>
      <span>绘制 {app.stats.frameMs.toFixed(1)} ms</span>
    </div>
  {/if}
</div>

<style>
  .stage {
    position: relative;
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    border-radius: var(--r-card);
    background: var(--tm-stage);
    border: 1px solid var(--card-stroke);
  }
  canvas {
    position: absolute;
    left: 0;
    top: 0;
    display: block;
    touch-action: manipulation;
  }
  /* the selection outline shows where the keyboard is; the stage edge only hints at focus */
  canvas:focus-visible {
    outline: none;
  }
  .stage:has(canvas:focus-visible) {
    border-color: var(--accent);
  }
  .hud {
    position: absolute;
    right: 10px;
    bottom: 10px;
    display: flex;
    gap: 10px;
    padding: 4px 10px;
    border-radius: var(--r-ctl);
    background: var(--acrylic);
    border: 1px solid var(--ctl-stroke);
    color: var(--text-2);
    pointer-events: none;
  }
  .hud .busy {
    color: var(--accent);
    font-weight: 600;
  }
</style>
