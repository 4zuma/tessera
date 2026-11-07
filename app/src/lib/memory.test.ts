import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MIN_GAP_MS, QUIET_MS, painted, reset } from './memory';

const g = globalThis as { gc?: () => void };

describe('memory: one collection once the treemap holds still', () => {
  let gc: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
    gc = vi.fn();
    g.gc = gc as unknown as () => void;
    reset();
  });

  afterEach(() => {
    reset();
    delete g.gc;
    vi.useRealTimers();
  });

  const still = () => painted(true, performance.now());
  const moving = () => painted(false, performance.now());

  it('collects after a frame that snaps without animating', () => {
    still(); // reduced motion or an instant layout: the only frame is already still
    vi.advanceTimersByTime(QUIET_MS - 1);
    expect(gc).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(gc).toHaveBeenCalledTimes(1);
  });

  it('waits while frames keep coming', () => {
    for (let i = 0; i < 100; i++) {
      moving();
      vi.advanceTimersByTime(16);
    }
    still();
    for (let i = 0; i < 10; i++) {
      vi.advanceTimersByTime(QUIET_MS / 2);
      still(); // a still frame now and then (a scan update with reduced motion)
    }
    expect(gc).not.toHaveBeenCalled();
    vi.advanceTimersByTime(QUIET_MS);
    expect(gc).toHaveBeenCalledTimes(1);
  });

  it('an idle window costs nothing', () => {
    still();
    vi.advanceTimersByTime(QUIET_MS);
    expect(gc).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(10 * MIN_GAP_MS);
    expect(gc).toHaveBeenCalledTimes(1);
  });

  it('keeps collections at least MIN_GAP_MS apart', () => {
    still();
    vi.advanceTimersByTime(QUIET_MS);
    expect(gc).toHaveBeenCalledTimes(1);
    still();
    vi.advanceTimersByTime(MIN_GAP_MS - 1);
    expect(gc).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);
    expect(gc).toHaveBeenCalledTimes(2);
  });

  it('does nothing without gc (a plain browser)', () => {
    delete g.gc;
    still();
    vi.advanceTimersByTime(MIN_GAP_MS);
    expect(vi.getTimerCount()).toBe(0);
  });
});
