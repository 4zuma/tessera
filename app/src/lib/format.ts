import { fmtNum, locale, t, tn } from './i18n.svelte';

export const KB = 1024;
export const MB = KB * 1024;
export const GB = MB * 1024;
export const TB = GB * 1024;

export function fmtBytes(b: number): string {
  if (!isFinite(b)) return '–';
  const s = b < 0 ? '−' : '';
  b = Math.abs(b);
  if (b >= TB) return s + (b / TB).toFixed(2) + ' TB';
  if (b >= GB) return s + (b / GB).toFixed(b >= 100 * GB ? 0 : 1) + ' GB';
  if (b >= MB) return s + (b / MB).toFixed(b >= 10 * MB ? 0 : 1) + ' MB';
  if (b >= KB) return s + Math.round(b / KB) + ' KB';
  return s + tn('fmt.bytes', Math.round(b));
}

export const fmtInt = (n: number) => fmtNum(Math.round(n));

export function fmtPct(p: number): string {
  const v = p * 100;
  if (v > 0 && v < 0.1) return '<0.1%';
  return (v >= 10 ? v.toFixed(0) : v.toFixed(1)) + '%';
}

export function fmtSecs(ms: number): string {
  const s = ms / 1000;
  if (s < 60) return t('fmt.sec', { s: fmtNum(s, 1) });
  return t('fmt.minSec', { m: fmtNum(Math.floor(s / 60)), s: fmtNum(Math.round(s % 60)) });
}

export function fmtDate(ms: number | null): string {
  if (ms == null) return '–';
  const d = new Date(ms);
  const days = (Date.now() - ms) / 864e5;
  const loc = locale();
  if (days < 1) return t('fmt.today', { time: new Intl.DateTimeFormat(loc, { hour: '2-digit', minute: '2-digit' }).format(d) });
  const date = new Intl.DateTimeFormat(loc, { year: 'numeric', month: 'numeric', day: 'numeric' }).format(d);
  if (days < 45) return tn('fmt.daysAgo', Math.round(days), { date });
  if (days < 365) return tn('fmt.monthsAgo', Math.round(days / 30), { date });
  const digits = days < 730 ? 1 : 0;
  return tn('fmt.yearsAgo', Number((days / 365).toFixed(digits)), { date }, digits);
}

/** a span of days as the age bands name it: 3 days, 2 weeks, 6 months, 5 years */
export function fmtSpan(days: number): string {
  if (days >= 365) return tn('age.years', Math.round(days / 365.25));
  if (days >= 28) return tn('age.months', Math.round(days / 30.44));
  if (days >= 7 && days % 7 === 0) return tn('age.weeks', days / 7);
  return tn('age.days', days);
}

/** an age band's name (see lib/age.ts › ageBand): "1 week – 1 month"; -1 is "not known" */
export function bandName(band: number, edges: readonly number[]): string {
  if (band < 0 || !edges.length) return t('age.unknown');
  if (band === 0) return t('age.within', { d: fmtSpan(edges[0]) });
  if (band >= edges.length) return t('age.over', { d: fmtSpan(edges[edges.length - 1]) });
  return t('age.between', { a: fmtSpan(edges[band - 1]), b: fmtSpan(edges[band]) });
}
