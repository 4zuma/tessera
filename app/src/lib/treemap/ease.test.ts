import { describe, expect, it } from 'vitest';
import { cubicBezier, FLUENT } from './ease';

/** y at x, sampled densely along the parametric curve (slow, exact enough for a test) */
function reference(x1: number, y1: number, x2: number, y2: number, x: number) {
  let best = 0;
  let err = Infinity;
  for (let i = 0; i <= 200000; i++) {
    const t = i / 200000;
    const u = 1 - t;
    const bx = 3 * u * u * t * x1 + 3 * u * t * t * x2 + t * t * t;
    if (Math.abs(bx - x) < err) {
      err = Math.abs(bx - x);
      best = 3 * u * u * t * y1 + 3 * u * t * t * y2 + t * t * t;
    }
  }
  return best;
}

describe('cubicBezier', () => {
  const curves: [string, [number, number, number, number]][] = [
    ['decelerate', [0, 0, 0, 1]],
    ['fastOut', [0.1, 0.9, 0.2, 1]],
    ['accelerate', [0.7, 0, 1, 0.5]],
    ['pane', [0, 0.35, 0.15, 1]],
    ['ease', [0.25, 0.1, 0.25, 1]],
  ];

  it('matches the curve everywhere, including the flat start of x1 = 0 curves', () => {
    for (const [name, c] of curves) {
      const f = cubicBezier(...c);
      for (const x of [0.001, 0.005, 0.015, 0.05, 0.1, 0.3, 0.5, 0.7, 0.9, 0.99]) {
        expect(f(x), `${name} at ${x}`).toBeCloseTo(reference(...c, x), 3);
      }
    }
  });

  it('never runs backwards', () => {
    for (const f of Object.values(FLUENT)) {
      let prev = 0;
      for (let i = 0; i <= 1000; i++) {
        const y = f(i / 1000);
        expect(y).toBeGreaterThanOrEqual(prev - 1e-9);
        prev = y;
      }
    }
  });
});
