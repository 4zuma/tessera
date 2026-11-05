export type Ease = (t: number) => number;

const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v);

/** CSS-style cubic-bezier timing function, solved with Newton steps. */
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
  return x => {
    if (x <= 0) return 0;
    if (x >= 1) return 1;
    let t = x;
    for (let i = 0; i < 8; i++) {
      const e = sx(t) - x;
      const d = dx(t);
      if (Math.abs(e) < 1e-5 || Math.abs(d) < 1e-6) break;
      t -= e / d;
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
};

export interface Motion {
  dur: number;
  ease: Ease;
}
