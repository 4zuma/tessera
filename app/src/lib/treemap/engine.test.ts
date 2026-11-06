import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Kind, type Layout, type Rect } from '../backend/codec';
import type { Theme } from '../theme';
import { nearlySame, TreemapEngine, type EngineHost, type Painter } from './engine';
import { fluentMotion } from './motion';

const raf = globalThis.requestAnimationFrame;
beforeAll(() => {
  // apply() only schedules a frame; none needs to run here
  globalThis.requestAnimationFrame = () => 1;
});
afterAll(() => {
  globalThis.requestAnimationFrame = raf;
});

function rect(id: number, x: number, y: number, w: number, h: number): Rect {
  return { id, parent: 0, target: 0, count: 0, x, y, w, h, size: 1000, name: '', depth: 1, kind: Kind.File, cat: 0, flags: 0, header: 0 };
}

const layout = (rects: Rect[]): Layout => ({ view: 0, rev: 1, bytesPerPx: 1, rects });

function engine() {
  const canvas = { getContext: () => ({}) } as unknown as HTMLCanvasElement;
  const host: EngineHost = { sel: new Set<number>(), hoverId: null, filter: null, scanning: false, reduced: false, theme: {} as Theme };
  const painter = { scene() {}, overlay() {}, inOverlay: () => false, light: 0, reset() {} } as Painter;
  return new TreemapEngine(canvas, host, painter, fluentMotion, 6);
}

describe('live layouts', () => {
  it('put sub-pixel moves in place without an animation', () => {
    const e = engine();
    e.apply(layout([rect(1, 10, 10, 200, 100), rect(2, 210, 10, 100, 100)]), 'instant');
    e.apply(layout([rect(1, 10, 10, 200.3, 100), rect(2, 210.3, 10, 99.7, 100)]), 'live');
    for (const id of [1, 2]) {
      const it = e.items.get(id)!;
      expect(it.done).toBe(true);
      expect(it.x + it.w).toBe(it.r.x + it.r.w);
    }
  });

  it('still animate moves you can see', () => {
    const e = engine();
    e.apply(layout([rect(1, 10, 10, 200, 100), rect(2, 210, 10, 100, 100)]), 'instant');
    e.apply(layout([rect(1, 10, 10, 203, 100), rect(2, 213, 10, 97, 100)]), 'live');
    expect(e.items.get(1)!.done).toBe(false);
    expect(e.items.get(2)!.done).toBe(false);
  });

  it('compare every edge', () => {
    const a = { x: 0, y: 0, w: 100, h: 50 };
    expect(nearlySame(a, { x: 0.4, y: -0.4, w: 99.7, h: 50.2 })).toBe(true);
    expect(nearlySame(a, { x: 0, y: 0, w: 100.6, h: 50 })).toBe(false);
    expect(nearlySame(a, { x: 0.3, y: 0, w: 100.3, h: 50 })).toBe(false); // right edge moved 0.6
  });
});
