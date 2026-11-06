export type Ease = (t: number) => number;

const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v);

/**
 * CSS-style cubic-bezier timing function: Newton steps, and bisection where they do not
 * converge. Curves that leave the start flat in x (x1 = 0, as Fluent's decelerate curves
 * do) have no slope to follow near t = 0, and Newton alone overshot there to t = 1, so the
 * first frame of a tween jumped to the end and back.
 */
export function cubicBezier(x1: number, y1: number, x2: number, y2: number): Ease {
  const cx = 3 * x1;
  const bx = 3 * (x2 - x1) - cx;
  const ax = 1 - cx - bx;
  const cy = 3 * y1;
  const by = 3 * (y2 - y1) - cy;
  const ay = 1 - cy - by;
  const sx = (t: number) => ((ax * t + bx) * t + cx) * t;
  const sy = (t: number) => ((ay * t + by) * t + cy) * t;
  const dx = (t: number) => (3 * ax * t + 2 * bx) * t + cx;
  const EPS = 1e-6;
  return x => {
    if (x <= 0) return 0;
    if (x >= 1) return 1;
    let t = x;
    for (let i = 0; i < 8; i++) {
      const e = sx(t) - x;
      if (Math.abs(e) < EPS) return sy(t);
      const d = dx(t);
      if (Math.abs(d) < EPS) break;
      const n = t - e / d;
      if (n < 0 || n > 1) break;
      t = n;
    }
    // x(t) rises monotonically on [0, 1] for timing curves
    let lo = 0;
    let hi = 1;
    t = x;
    for (let i = 0; i < 40; i++) {
      const e = sx(t) - x;
      if (Math.abs(e) < EPS) break;
      if (e < 0) lo = t;
      else hi = t;
      t = (lo + hi) / 2;
    }
    return sy(clamp01(t));
  };
}

/** Windows 11 motion curves (WinUI ControlFastOutSlowIn / Decelerate / Accelerate). */
export const FLUENT = {
  decelerate: cubicBezier(0, 0, 0, 1),
  fastOut: cubicBezier(0.1, 0.9, 0.2, 1),
  accelerate: cubicBezier(0.7, 0, 1, 0.5),
  /** point-to-point, used for zoom: soft start, long settle */
  pointToPoint: cubicBezier(0.55, 0.55, 0, 1),
  /** WinUI SplitView's inline pane: KeySpline 0,0.35 0.15,1 (opens in 200 ms, closes in 100 ms) */
  pane: cubicBezier(0, 0.35, 0.15, 1),
};

export interface Motion {
  dur: number;
  ease: Ease;
}
