/**
 * How long ago something was modified, kept as one byte per node (the core's
 * crates/tessera-core/src/age.rs has the same scale): 0 = not known, 1 = within a day
 * (or in the future), then a log scale up to 255 = 30 years or more, about 3.7 % per step.
 * A folder's or a group's byte is the size-weighted mean over the files in it.
 */
export const AGE_UNKNOWN = 0;
const MAX_DAYS = 30 * 365.25;
/** ln(days) per step */
const STEP = Math.log(MAX_DAYS) / 254;

/** the age byte of something modified `days` days ago */
export function ageByte(days: number): number {
  if (!Number.isFinite(days)) return AGE_UNKNOWN;
  if (days < 1) return 1;
  return Math.min(255, 1 + Math.floor(Math.log(days) / STEP));
}

/** the age a byte stands for, in days (the start of its step); NaN when not known */
export function ageDays(b: number): number {
  if (b <= AGE_UNKNOWN) return NaN;
  if (b === 1) return 0;
  return Math.exp((b - 1) * STEP);
}

/** band edges a user can pick from, in days */
export const AGE_EDGE_CHOICES = [1, 3, 7, 14, 30, 91, 182, 365, 730, 1095, 1826, 3652];
/** the default bands: a week, a month, a year, three years */
export const AGE_EDGES = [7, 30, 365, 1095];

/** which band an age byte falls in (0 = newest), or -1 when its age is not known */
export function ageBand(b: number, edges: readonly number[]): number {
  if (b <= AGE_UNKNOWN) return -1;
  const d = ageDays(b);
  let i = 0;
  while (i < edges.length && d >= edges[i]) i++;
  return i;
}

/**
 * The legend's bins (Backend.ageStats): 0 = not known, then one per span between two
 * neighbouring edge choices, so the bytes of any bands drawn from the choices are sums of
 * bins. The core has the same table.
 */
export const AGE_BINS = AGE_EDGE_CHOICES.length + 2;

/** the bin of an age byte */
export function ageBin(b: number): number {
  if (b <= AGE_UNKNOWN) return 0;
  const d = ageDays(b);
  let i = 0;
  while (i < AGE_EDGE_CHOICES.length && d >= AGE_EDGE_CHOICES[i]) i++;
  return i + 1;
}

/** bytes per band from bytes per bin; `edges` ascending and among AGE_EDGE_CHOICES */
export function bandTotals(bins: readonly number[], edges: readonly number[]): number[] {
  const out = new Array<number>(edges.length + 1).fill(0);
  for (let j = 1; j < bins.length; j++) {
    if (!bins[j]) continue;
    const from = j === 1 ? 0 : AGE_EDGE_CHOICES[j - 2];
    let band = 0;
    while (band < edges.length && from >= edges[band]) band++;
    out[band] += bins[j];
  }
  return out;
}
