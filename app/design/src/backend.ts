/**
 * Stands in for src/lib/backend/index.ts in the design-system build (vite.config.ts swaps the
 * module): always the mock backend, never Tauri, and scans that either finish at once or stay
 * frozen part way, so a preview opens on the state it shows instead of a 5 s scan animation.
 */
import { MockBackend } from '../../src/lib/backend/mock';
import type { Backend } from '../../src/lib/backend/types';
import { config } from './config';

export * from '../../src/lib/backend/types';

export const inTauri = () => false;

/** a frozen scan pretends to take this long, so its elapsed time reads like a real one */
const FROZEN_SCAN_MS = 20000;
/**
 * an "instant" scan: done on the mock's third progress tick. Not shorter: the app ignores
 * progress of a scan it has not been told the id of yet, so a scan that ends on its very first
 * tick would look like it never ends.
 */
const QUICK_SCAN_MS = 250;

interface MockInternals {
  scanMs: number;
  scanStartAt: number;
  scanTimer: ReturnType<typeof setInterval> | null;
  tickScan(scanId: number): void;
}

export async function createBackend(): Promise<Backend> {
  const lang = config.lang ? { lang: config.lang, locale: config.lang } : {};
  const b = new MockBackend(lang, { scanMs: QUICK_SCAN_MS });
  const x = b as unknown as MockInternals;
  const start = b.scanStart.bind(b);
  b.scanStart = async root => {
    const f = config.freezeAt;
    x.scanMs = f == null ? QUICK_SCAN_MS : FROZEN_SCAN_MS;
    const s = await start(root);
    if (f != null) {
      // jump to the share asked for, show it, and stop the clock: the scan stays "running"
      x.scanStartAt = performance.now() - f * x.scanMs;
      if (x.scanTimer) clearInterval(x.scanTimer);
      x.scanTimer = null;
      // after the app has taken the scan's id (it does so when this returns)
      setTimeout(() => x.tickScan(s.scanId), 0);
    }
    return s;
  };
  return b;
}
