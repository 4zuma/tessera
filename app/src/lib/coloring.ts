/**
 * What the map is coloured by: file type (the `cat` byte) or age (the `age` byte, in bands
 * the user can move). The painter and the legend filter both go through `key`.
 */
import { ageBand } from './age';
import type { Rect } from './backend/codec';
import { ageBases, tones, type Theme } from './theme';

export type ColorBy = 'type' | 'age';

export interface Coloring {
  by: ColorBy;
  /** a tile's legend entry: its category or its age band; outside 0..fill.length - 1 = no colour */
  key(r: Rect): number;
  fill: readonly string[];
  ink: readonly string[];
  tint: readonly string[];
}

export function makeColoring(by: ColorBy, theme: Theme, edges: readonly number[]): Coloring {
  if (by === 'type') return { by, key: r => r.cat, fill: theme.fill, ink: theme.ink, tint: theme.tint };
  const e = [...edges];
  return { by, key: r => ageBand(r.age, e), ...tones(theme.scheme, ageBases(e.length + 1)) };
}
