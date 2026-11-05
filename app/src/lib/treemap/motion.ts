import type { MotionSet } from './engine';
import { FLUENT } from './ease';

/** Windows 11 timing: 167 / 250 / 333 ms steps, decelerate in, accelerate out. */
export const fluentMotion: MotionSet = {
  zoom: { dur: 420, ease: FLUENT.fastOut },
  update: { dur: 333, ease: FLUENT.decelerate },
  scan: { dur: 560, ease: FLUENT.fastOut },
  live: { dur: 620, ease: FLUENT.fastOut },
  morph: { dur: 520, ease: FLUENT.fastOut },
  mode: { dur: 460, ease: FLUENT.fastOut },
  delete: { dur: 333, ease: FLUENT.decelerate },
  instant: { dur: 1, ease: t => t },
  remove: { dur: 167, ease: FLUENT.accelerate },
  reflow: { dur: 333, ease: FLUENT.decelerate },
  reflowDelay: 110,
  appear: { dur: 333, ease: FLUENT.decelerate },
  appearFrom: r => ({ x: r.x + r.w * 0.04, y: r.y + r.h * 0.04, w: r.w * 0.92, h: r.h * 0.92 }),
  removeTo: r => ({ x: r.x + r.w * 0.1, y: r.y + r.h * 0.1, w: r.w * 0.8, h: r.h * 0.8 }),
};

/** layout parameters for the Fluent look (sent to the core with every layout request) */
export const fluentLayout = {
  outer: 6,
  pad: 3,
  gap: 2,
  header: 21,
  minHeaderW: 60,
  minArea: 110,
  recurse: 3,
  maxDepth: 14,
  maxPerDir: 400,
  labelMinW: 44,
  labelMinH: 20,
};
