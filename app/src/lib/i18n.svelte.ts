/**
 * UI language. There is no language setting: Rust picks the language once at startup
 * from the Windows "Preferred languages" list (app/src-tauri/src/i18n.rs) and hands it
 * over in `platform_info`; the browser preview picks it from `navigator.languages` the
 * same way. The first language with a translation wins, else English. A key that a
 * dictionary lacks shows its English text.
 */
import { en, type Dict, type Key, type Msg } from './locales/en';
import { zhCN } from './locales/zh-CN';

export type Lang = 'en' | 'zh-CN';
export type { Key };
type Vars = Record<string, string | number>;

export const DICTS: Record<Lang, Partial<Dict>> = { en, 'zh-CN': zhCN };

export interface LangChoice {
  lang: Lang;
  /** BCP 47 tag for dates and numbers, e.g. en-GB */
  locale: string;
}

/**
 * `en-*` is English; Chinese counts when it is Simplified: `zh-Hans-*`, or `zh`, `zh-CN`,
 * `zh-SG`… without a script. Traditional Chinese has no translation yet. Same rules as
 * `match_tag` in i18n.rs.
 */
export function matchTag(tag: string): Lang | null {
  const [primary, ...rest] = tag.trim().toLowerCase().replace(/_/g, '-').split('-');
  if (primary === 'en') return 'en';
  if (primary !== 'zh') return null;
  const subtags: string[] = [];
  for (const p of rest) {
    if (p.length <= 1) break; // an extension (-u-…, -x-…) starts here
    subtags.push(p);
  }
  if (subtags.includes('hans')) return 'zh-CN';
  if (subtags.some(p => p === 'hant' || p === 'tw' || p === 'hk' || p === 'mo')) return null;
  return 'zh-CN';
}

/** The first tag in `prefs` with a translation, else English. */
export function resolve(prefs: readonly string[]): LangChoice {
  for (const tag of prefs) {
    const lang = matchTag(tag);
    if (!lang) continue;
    if (lang === 'zh-CN') return { lang, locale: 'zh-CN' };
    return { lang, locale: validLocale(tag.trim().replace(/_/g, '-')) ?? 'en-US' };
  }
  return { lang: 'en', locale: 'en-US' };
}

/** What a plain browser would pick (the preview, or when Rust could not be asked). */
export function browserChoice(): LangChoice {
  if (typeof navigator === 'undefined') return resolve([]);
  return resolve(navigator.languages?.length ? navigator.languages : [navigator.language ?? '']);
}

function validLocale(tag: string): string | null {
  try {
    return Intl.getCanonicalLocales(tag)[0] ?? null;
  } catch {
    return null;
  }
}

const cur = $state<LangChoice>({ lang: 'en', locale: 'en-US' });
let plural = new Intl.PluralRules('en-US');

/** Switch the UI language; unknown languages and bad tags fall back to English. */
export function setLang(lang: string | null | undefined, locale?: string | null) {
  const l: Lang = lang && Object.hasOwn(DICTS, lang) ? (lang as Lang) : 'en';
  const loc = (locale && matchTag(locale) === l && validLocale(locale)) || (l === 'en' ? 'en-US' : l);
  cur.lang = l;
  cur.locale = loc;
  plural = new Intl.PluralRules(loc);
  if (typeof document !== 'undefined') document.documentElement.lang = loc;
}

export const lang = (): Lang => cur.lang;
export const locale = (): string => cur.locale;

function msg(key: Key): Msg {
  return DICTS[cur.lang][key] ?? en[key];
}

function fill(s: string, vars?: Vars): string {
  if (!vars) return s;
  return s.replace(/\{(\w+)\}/g, (m, k: string) => (Object.hasOwn(vars, k) ? String(vars[k]) : m));
}

/** Integer with the locale's digit grouping. */
export function fmtNum(n: number, digits = 0): string {
  return n.toLocaleString(cur.locale, { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

/** The text for `key` with `{name}` placeholders filled in. */
export function t(key: Key, vars?: Vars): string {
  const m = msg(key);
  return fill(typeof m === 'string' ? m : m.other, vars);
}

/**
 * Text that counts something: picks the plural form for `n` and fills `{n}` with it
 * (shown with `digits` decimals, which also decide the form: "1.0 years").
 */
export function tn(key: Key, n: number, vars?: Vars, digits = 0): string {
  const m = msg(key);
  let s: string;
  if (typeof m === 'string') s = m;
  else {
    const rules = digits ? new Intl.PluralRules(cur.locale, { minimumFractionDigits: digits }) : plural;
    s = (rules.select(n) === 'one' && m.one) || m.other;
  }
  return fill(s, { n: fmtNum(n, digits), ...vars });
}

export interface Part {
  text: string;
  /** the placeholder this part fills, for parts that are values */
  v?: string;
}

/** `t()` split into literal text and values, for markup that sets the values apart (bold). */
export function tParts(key: Key, vars: Vars): Part[] {
  const m = msg(key);
  const s = typeof m === 'string' ? m : m.other;
  const out: Part[] = [];
  let last = 0;
  for (const x of s.matchAll(/\{(\w+)\}/g)) {
    if (!Object.hasOwn(vars, x[1])) continue;
    if (x.index > last) out.push({ text: s.slice(last, x.index) });
    out.push({ text: String(vars[x[1]]), v: x[1] });
    last = x.index + x[0].length;
  }
  if (last < s.length) out.push({ text: s.slice(last) });
  return out;
}

/** Joins a few names the way the language lists them. */
export const joinList = (names: string[]) => names.join(t('listSep'));

/** A file category's name, or undefined for ids outside the table. */
export function catName(cat: number): string | undefined {
  const key = `cat.${cat}`;
  return Object.hasOwn(en, key) ? t(key as Key) : undefined;
}

// the best guess until Rust answers; nothing is shown before then
setLang(browserChoice().lang, browserChoice().locale);
