/**
 * Windows 11 (WinUI 3) design tokens. One source for the CSS (written as custom
 * properties on <html>) and for the canvas painter.
 */
import { CAT_BASE, CAT_COUNT } from './backend/categories';

export type Scheme = 'light' | 'dark';

const LIGHT = {
  'solid-base': '#f3f3f3',
  'mica-a': '#eef1f7',
  'mica-b': '#f5f1ef',
  layer: 'rgba(255,255,255,0.5)',
  card: 'rgba(255,255,255,0.7)',
  'card-2': 'rgba(246,246,246,0.5)',
  'card-stroke': 'rgba(0,0,0,0.0578)',
  ctl: 'rgba(255,255,255,0.7)',
  'ctl-hover': 'rgba(249,249,249,0.5)',
  'ctl-press': 'rgba(249,249,249,0.3)',
  'ctl-stroke': 'rgba(0,0,0,0.0578)',
  'ctl-stroke-2': 'rgba(0,0,0,0.1622)',
  'ctl-strong': 'rgba(0,0,0,0.4458)',
  // ControlFillColorInputActive: a text box that has the focus
  'ctl-input-active': '#ffffff',
  // ControlAltFillColorSecondary: the sunken track of a segmented control or toggle
  'ctl-alt': 'rgba(0,0,0,0.0241)',
  'subtle-hover': 'rgba(0,0,0,0.0373)',
  'subtle-press': 'rgba(0,0,0,0.0241)',
  divider: 'rgba(0,0,0,0.0803)',
  text: 'rgba(0,0,0,0.896)',
  'text-2': 'rgba(0,0,0,0.606)',
  'text-3': 'rgba(0,0,0,0.446)',
  'text-dis': 'rgba(0,0,0,0.361)',
  accent: '#005fb8',
  'on-accent': '#ffffff',
  critical: '#c42b1c',
  'critical-bg': '#fde7e9',
  'on-critical': '#ffffff',
  caution: '#9d5d00',
  acrylic: 'rgba(252,252,252,0.85)',
  'acrylic-solid': '#f9f9f9',
  // ContentDialog: content area over the solid base, command area on the base
  'dlg-top': '#ffffff',
  'dlg-stroke': 'rgba(117,117,117,0.4)',
  smoke: 'rgba(0,0,0,0.3)',
  shadow: '0 8px 16px rgba(0,0,0,0.14)',
  'shadow-dialog': '0 32px 64px rgba(0,0,0,0.1876), 0 2px 21px rgba(0,0,0,0.1474)',
  // treemap
  'tm-stage': 'rgba(255,255,255,0.42)',
  'tm-folder': 'rgba(255,255,255,0.46)',
  'tm-folder-head': 'rgba(0,0,0,0.028)',
  'tm-folder-text': 'rgba(0,0,0,0.80)',
  'tm-leafdir': 'rgba(255,255,255,0.66)',
  'tm-leafdir-stroke': 'rgba(0,0,0,0.14)',
  'tm-leafdir-ink': 'rgba(0,0,0,0.72)',
  'tm-hover': 'rgba(255,255,255,0.38)',
  'tm-reveal': 'rgba(0,95,184,0.55)',
  'tm-stripe': 'rgba(0,0,0,0.05)',
  'tm-dots': 'rgba(255,255,255,0.6)',
  'tm-hatch': 'rgba(255,255,255,0.5)',
};

type Tokens = typeof LIGHT;

const DARK: Tokens = {
  'solid-base': '#202020',
  'mica-a': '#1c1f26',
  'mica-b': '#232021',
  layer: 'rgba(58,58,58,0.3)',
  card: 'rgba(255,255,255,0.0512)',
  'card-2': 'rgba(255,255,255,0.0326)',
  'card-stroke': 'rgba(0,0,0,0.1)',
  ctl: 'rgba(255,255,255,0.0605)',
  'ctl-hover': 'rgba(255,255,255,0.0837)',
  'ctl-press': 'rgba(255,255,255,0.0326)',
  'ctl-stroke': 'rgba(255,255,255,0.0698)',
  'ctl-stroke-2': 'rgba(255,255,255,0.093)',
  'ctl-strong': 'rgba(255,255,255,0.5442)',
  'ctl-input-active': 'rgba(30,30,30,0.7)',
  'ctl-alt': 'rgba(0,0,0,0.1)',
  'subtle-hover': 'rgba(255,255,255,0.0605)',
  'subtle-press': 'rgba(255,255,255,0.0419)',
  divider: 'rgba(255,255,255,0.0837)',
  text: '#ffffff',
  'text-2': 'rgba(255,255,255,0.786)',
  'text-3': 'rgba(255,255,255,0.5442)',
  'text-dis': 'rgba(255,255,255,0.3628)',
  accent: '#60cdff',
  'on-accent': '#000000',
  critical: '#ff99a4',
  'critical-bg': '#442726',
  'on-critical': '#1a0003',
  caution: '#fce100',
  acrylic: 'rgba(44,44,44,0.96)',
  'acrylic-solid': '#2c2c2c',
  'dlg-top': 'rgba(255,255,255,0.0538)',
  'dlg-stroke': 'rgba(117,117,117,0.4)',
  smoke: 'rgba(0,0,0,0.3)',
  shadow: '0 8px 16px rgba(0,0,0,0.26)',
  'shadow-dialog': '0 32px 64px rgba(0,0,0,0.37), 0 2px 21px rgba(0,0,0,0.37)',
  'tm-stage': 'rgba(0,0,0,0.16)',
  'tm-folder': 'rgba(255,255,255,0.035)',
  'tm-folder-head': 'rgba(255,255,255,0.03)',
  'tm-folder-text': 'rgba(255,255,255,0.88)',
  'tm-leafdir': 'rgba(255,255,255,0.075)',
  'tm-leafdir-stroke': 'rgba(255,255,255,0.15)',
  'tm-leafdir-ink': 'rgba(255,255,255,0.84)',
  'tm-hover': 'rgba(255,255,255,0.11)',
  'tm-reveal': 'rgba(255,255,255,0.7)',
  'tm-stripe': 'rgba(255,255,255,0.06)',
  'tm-dots': 'rgba(0,0,0,0.38)',
  'tm-hatch': 'rgba(0,0,0,0.28)',
};

export interface Theme {
  scheme: Scheme;
  tok: Tokens;
  /** fill and label ink per file category */
  fill: string[];
  ink: string[];
  /** a light wash per category for closed folders, by what fills them most */
  tint: string[];
  /** the blend behind .acrylic surfaces, see {@link acrylicBlend} */
  acrylic: { matrix: string; base: string };
}

/**
 * WinUI's default acrylic (AcrylicInAppFillColorDefaultBrush, which ToolTip uses; MenuFlyout's
 * DesktopAcrylicBackdrop has the same colours). In microsoft-ui-xaml's AcrylicBrush the blurred
 * backdrop (30 px) takes the tint's luminosity at TintLuminosityOpacity, then the tint's colour
 * at TintOpacity, then 2% noise.
 *
 * `chroma` stands in for the clipping of the luminosity blend: near white (light theme) most
 * colours fall out of gamut and get pulled toward grey, which a colour matrix cannot do, so it
 * scales colour by a fixed amount instead. 0.5 fits the exact blend over the treemap to about
 * 1/255 on average; in the dark theme nothing clips and the matrix is exact.
 */
const ACRYLIC: Record<Scheme, { tint: string; tintOpacity: number; luminosityOpacity: number; chroma: number }> = {
  light: { tint: '#fcfcfc', tintOpacity: 0, luminosityOpacity: 0.85, chroma: 0.5 },
  dark: { tint: '#2c2c2c', tintOpacity: 0.15, luminosityOpacity: 0.96, chroma: 1 },
};

/** luma weights of the W3C / Direct2D luminosity and colour blend modes */
const LUMA = [0.3, 0.59, 0.11];

/**
 * The acrylic blend as one feColorMatrix (sRGB, unpremultiplied).
 *
 * Luminosity blend at opacity a with a grey of luma l: x + a·(l − Y(x)).
 * Colour blend with that grey at opacity t: y − t·(y − Y(y)).
 * Together: Y'= (1−a)·Y(x) + a·l, colour (x − Y(x)) scaled by (1−t)·chroma.
 */
export function acrylicBlend(scheme: Scheme): { matrix: string; base: string } {
  const r = ACRYLIC[scheme];
  const a = r.luminosityOpacity;
  const l = hexToRgb(r.tint)[0] / 255;
  const k = (1 - r.tintOpacity) * r.chroma;
  const rows: string[] = [];
  for (let i = 0; i < 3; i++) {
    const row = LUMA.map((w, j) => k * ((i === j ? 1 : 0) - w) + (1 - a) * w);
    rows.push([...row, 0, a * l].map(v => +v.toFixed(4)).join(' '));
  }
  rows.push('0 0 0 1 0');
  return { matrix: rows.join('  '), base: r.tint };
}

function hexToRgb(h: string): [number, number, number] {
  h = h.replace('#', '');
  if (h.length === 3) h = h.split('').map(c => c + c).join('');
  const n = parseInt(h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
export function mix(a: string, b: string, t: number): string {
  const A = hexToRgb(a);
  const B = hexToRgb(b);
  return '#' + A.map((v, i) => Math.round(v + (B[i] - v) * t).toString(16).padStart(2, '0')).join('');
}

const toLin = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const toSrgb = (v: number) => (v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055);

function toLab(h: string): [number, number, number] {
  const [r, g, b] = hexToRgb(h).map(v => toLin(v / 255));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s, 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s, 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s];
}

/** linear sRGB of an OKLab colour (may fall outside 0..1) */
function labToLin([L, A, B]: [number, number, number]): [number, number, number] {
  const l = (L + 0.3963377774 * A + 0.2158037573 * B) ** 3;
  const m = (L - 0.1055613458 * A - 0.0638541728 * B) ** 3;
  const s = (L - 0.0894841775 * A - 1.291485548 * B) ** 3;
  return [4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s, -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s, -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s];
}

function fromLab(lab: [number, number, number]): string {
  return '#' + labToLin(lab).map(v => Math.round(Math.max(0, Math.min(1, toSrgb(v))) * 255).toString(16).padStart(2, '0')).join('');
}

/** OKLCH: lightness, chroma, hue in radians */
function toLch(h: string): [number, number, number] {
  const [L, a, b] = toLab(h);
  return [L, Math.hypot(a, b), Math.atan2(b, a)];
}

const inGamut = (lab: [number, number, number]) => labToLin(lab).every(v => v > -1e-4 && v < 1 + 1e-4);

/** an OKLCH colour, with its chroma cut to the most sRGB can show at that lightness and hue */
function fromLch(L: number, C: number, H: number): string {
  const at = (c: number): [number, number, number] => [L, c * Math.cos(H), c * Math.sin(H)];
  let lo = 0;
  let hi = C;
  if (inGamut(at(C))) lo = C;
  else
    for (let i = 0; i < 20; i++) {
      const c = (lo + hi) / 2;
      if (inGamut(at(c))) lo = c;
      else hi = c;
    }
  return fromLab(at(lo));
}

/**
 * `a` blended toward `b` by `t` in OKLCH: lightness and chroma move evenly and the hue turns
 * the shorter way round, so the colours in between stay as vivid as the ends (a blend in
 * OKLab or sRGB goes grey in the middle when the hues are far apart).
 */
export function mixLch(a: string, b: string, t: number): string {
  const [L1, C1, H1] = toLch(a);
  const [L2, C2, H2] = toLch(b);
  let d = H2 - H1;
  if (d > Math.PI) d -= 2 * Math.PI;
  if (d < -Math.PI) d += 2 * Math.PI;
  return fromLch(L1 + (L2 - L1) * t, C1 + (C2 - C1) * t, H1 + d * t);
}

export function rgba(h: string, a: number): string {
  const [r, g, b] = hexToRgb(h);
  return `rgba(${r},${g},${b},${a})`;
}

/** a tile fill, its label ink and the wash of a closed folder, per base colour */
export interface Tones {
  fill: string[];
  ink: string[];
  tint: string[];
}

/** the painter's tones of some base colours: mixed toward the surface, as the categories are */
export function tones(scheme: Scheme, bases: readonly string[]): Tones {
  const fill: string[] = [];
  const ink: string[] = [];
  const tint: string[] = [];
  for (const base of bases) {
    fill.push(scheme === 'dark' ? mix(base, '#202020', 0.42) : mix(base, '#ffffff', 0.52));
    ink.push(scheme === 'dark' ? mix(base, '#ffffff', 0.82) : mix(base, '#000000', 0.62));
    tint.push(scheme === 'dark' ? mix(base, '#202020', 0.74) : mix(base, '#ffffff', 0.8));
  }
  return { fill, ink, tint };
}

/**
 * Colour by age: newest warm, oldest cold, red to blue from the Windows palette. Only the two
 * ends are set, so any number of bands spreads evenly between them without getting loud.
 */
export const AGE_RAMP: readonly string[] = ['#D13438', '#0063B1'];

/** `n` base colours along {@link AGE_RAMP}, newest first (blended in OKLCH: red, rose, purple, violet, blue) */
export function ageBases(n: number): string[] {
  const k = AGE_RAMP.length - 1;
  const out: string[] = [];
  for (let i = 0; i < n; i++) {
    const p = n === 1 ? 0 : (i * k) / (n - 1);
    const j = Math.min(k - 1, Math.floor(p));
    const f = p - j;
    out.push(f < 1e-6 ? AGE_RAMP[j] : f > 1 - 1e-6 ? AGE_RAMP[j + 1] : mixLch(AGE_RAMP[j], AGE_RAMP[j + 1], f));
  }
  return out;
}

export function makeTheme(scheme: Scheme, accent: { light: string; dark: string } | null): Theme {
  const tok: Tokens = { ...(scheme === 'dark' ? DARK : LIGHT) };
  if (accent) tok.accent = scheme === 'dark' ? accent.dark : accent.light;
  tok['tm-reveal'] = scheme === 'dark' ? 'rgba(255,255,255,0.7)' : rgba(tok.accent, 0.55);
  const { fill, ink, tint } = tones(scheme, CAT_BASE.slice(0, CAT_COUNT));
  return { scheme, tok, fill, ink, tint, acrylic: acrylicBlend(scheme) };
}

export function applyThemeCss(t: Theme, el: HTMLElement = document.documentElement) {
  for (const [k, v] of Object.entries(t.tok)) el.style.setProperty('--' + k, v);
  el.style.setProperty('--accent-soft', rgba(t.tok.accent, t.scheme === 'dark' ? 0.14 : 0.1));
  el.style.setProperty('--accent-hover', rgba(t.tok.accent, 0.9));
  el.style.setProperty('--accent-press', rgba(t.tok.accent, 0.8));
  t.fill.forEach((f, i) => el.style.setProperty(`--cat-${i}`, f));
  el.dataset.scheme = t.scheme;
  el.style.colorScheme = t.scheme;
}
