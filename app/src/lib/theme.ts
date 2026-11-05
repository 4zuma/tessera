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
export function rgba(h: string, a: number): string {
  const [r, g, b] = hexToRgb(h);
  return `rgba(${r},${g},${b},${a})`;
}

export function makeTheme(scheme: Scheme, accent: { light: string; dark: string } | null): Theme {
  const tok: Tokens = { ...(scheme === 'dark' ? DARK : LIGHT) };
  if (accent) tok.accent = scheme === 'dark' ? accent.dark : accent.light;
  tok['tm-reveal'] = scheme === 'dark' ? 'rgba(255,255,255,0.7)' : rgba(tok.accent, 0.55);
  const fill: string[] = [];
  const ink: string[] = [];
  for (let c = 0; c < CAT_COUNT; c++) {
    const base = CAT_BASE[c];
    fill.push(scheme === 'dark' ? mix(base, '#202020', 0.42) : mix(base, '#ffffff', 0.52));
    ink.push(scheme === 'dark' ? mix(base, '#ffffff', 0.82) : mix(base, '#000000', 0.62));
  }
  return { scheme, tok, fill, ink };
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
