/**
 * Gives garbage back once the treemap holds still.
 *
 * Every frame and every layout leaves short-lived garbage: JS objects, and canvas and IPC
 * objects in Blink's heap. While memory is plentiful, WebView2 lets it pile up until the
 * next burst of allocations, so after a scan the page process held ~30 MB it did not need.
 * Two things keep it small (measured in Chromium with the mock backend: page process
 * 97 -> 62 MB after a scan, 141 -> 106 MB in a maximized 4K window):
 * - WebView2 starts with `--js-flags="--max-semi-space-size=1 --expose-gc"`
 *   (tauri.conf.json): V8's young generation stays at 1 MB instead of growing to 16 MB;
 * - one full collection (`gc()`, which also sweeps Blink's heap) once no frame has been
 *   painted for QUIET_MS after one that left everything still, at most every MIN_GAP_MS,
 *   and only if frames ran since the last one, so an idle window costs nothing.
 *   Any frame counts, also one that snaps without animating (reduced motion, an instant
 *   layout).
 * In a plain browser `gc` does not exist and this does nothing.
 */

export const QUIET_MS = 1500;
export const MIN_GAP_MS = 15000;

type Gc = () => void;

let timer: ReturnType<typeof setTimeout> | null = null;
let last = -Infinity;
let pending = false;

function collector(): Gc | null {
  const g = (globalThis as { gc?: unknown }).gc;
  return typeof g === 'function' ? (g as Gc) : null;
}

/**
 * A frame was painted. `still`: nothing moves after it, so collect once no other frame
 * follows for QUIET_MS; otherwise hold the collection back.
 */
export function painted(still: boolean, now = performance.now()) {
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
  pending = true;
  if (still && collector()) timer = setTimeout(run, Math.max(QUIET_MS, last + MIN_GAP_MS - now));
}

function run() {
  timer = null;
  const g = collector();
  if (!pending || !g) return;
  pending = false;
  last = performance.now();
  g();
}

/** for tests: forget all state */
export function reset() {
  if (timer) clearTimeout(timer);
  timer = null;
  last = -Infinity;
  pending = false;
}
