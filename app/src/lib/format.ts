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
  return s + Math.round(b) + ' 字节';
}

export const fmtInt = (n: number) => Math.round(n).toLocaleString('zh-CN');

export function fmtPct(p: number): string {
  const v = p * 100;
  if (v > 0 && v < 0.1) return '<0.1%';
  return (v >= 10 ? v.toFixed(0) : v.toFixed(1)) + '%';
}

export function fmtSecs(ms: number): string {
  const s = ms / 1000;
  if (s < 60) return s.toFixed(1) + ' 秒';
  return `${Math.floor(s / 60)} 分 ${Math.round(s % 60)} 秒`;
}

export function fmtDate(ms: number | null): string {
  if (ms == null) return '–';
  const d = new Date(ms);
  const days = (Date.now() - ms) / 864e5;
  const ymd = `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()}`;
  if (days < 1) return `今天 ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  if (days < 45) return `${ymd}（${Math.round(days)} 天前）`;
  if (days < 365) return `${ymd}（${Math.round(days / 30)} 个月前）`;
  return `${ymd}（${(days / 365).toFixed(days < 730 ? 1 : 0)} 年前）`;
}
