import { describe, expect, it } from 'vitest';
import { acrylicBlend } from './theme';

type Rgb = [number, number, number];
const lum = (c: Rgb) => 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];

/** W3C / Direct2D SetLum with ClipColor */
function setLum(c: Rgb, l: number): Rgb {
  const d = l - lum(c);
  let o = c.map(v => v + d) as Rgb;
  const L = lum(o);
  const n = Math.min(...o);
  const x = Math.max(...o);
  if (n < 0) o = o.map(v => L + ((v - L) * L) / (L - n)) as Rgb;
  if (x > 1) o = o.map(v => L + ((v - L) * (1 - L)) / (x - L)) as Rgb;
  return o;
}

/** WinUI's recipe: luminosity blend at lumOpacity, then colour blend at tintOpacity */
function acrylic(x: Rgb, tint: number, lumOpacity: number, tintOpacity: number): Rgb {
  const y = setLum(x, tint).map((v, i) => (1 - lumOpacity) * x[i] + lumOpacity * v) as Rgb;
  const g = lum(y);
  return y.map(v => (1 - tintOpacity) * v + tintOpacity * g) as Rgb;
}

function apply(matrix: string, x: Rgb): Rgb {
  const m = matrix.trim().split(/\s+/).map(Number);
  expect(m).toHaveLength(20);
  return [0, 1, 2].map(r => m[r * 5] * x[0] + m[r * 5 + 1] * x[1] + m[r * 5 + 2] * x[2] + m[r * 5 + 4]) as Rgb;
}

const hex = (h: string): Rgb => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16) / 255) as Rgb;

describe('acrylicBlend', () => {
  it('matches the WinUI blend in the dark theme', () => {
    const { matrix, base } = acrylicBlend('dark');
    expect(base).toBe('#2c2c2c');
    // treemap fills of the dark theme, a grey and white text
    for (const c of ['#5c5a85', '#25586a', '#7b4a2f', '#383838', '#ffffff', '#000000']) {
      const want = acrylic(hex(c), 44 / 255, 0.96, 0.15);
      const got = apply(matrix, hex(c));
      got.forEach((v, i) => expect(v).toBeCloseTo(want[i], 3));
    }
  });

  it('keeps the luminosity of the WinUI blend in the light theme', () => {
    const { matrix, base } = acrylicBlend('light');
    expect(base).toBe('#fcfcfc');
    for (const c of ['#c4c3eb', '#80cce0', '#e5a888', '#808080', '#000000']) {
      const want = acrylic(hex(c), 252 / 255, 0.85, 0);
      const got = apply(matrix, hex(c)).map(v => Math.min(1, Math.max(0, v))) as Rgb;
      // within 4/255 once the matrix output is clamped
      expect(Math.abs(lum(got) - lum(want))).toBeLessThan(0.016);
      // colour is kept but softened, as the clipped blend does; these are unblurred fills, the
      // worst case (over the blurred treemap the mean error is about 1/255)
      got.forEach((v, i) => expect(Math.abs(v - want[i])).toBeLessThan(0.07));
    }
  });

  it('leaves greys grey', () => {
    for (const s of ['light', 'dark'] as const) {
      const [r, g, b] = apply(acrylicBlend(s).matrix, [0.5, 0.5, 0.5]);
      expect(r).toBeCloseTo(g, 6);
      expect(g).toBeCloseTo(b, 6);
    }
  });
});
