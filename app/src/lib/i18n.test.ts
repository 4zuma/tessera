import { afterEach, describe, expect, it } from 'vitest';
import { fmtBytes, fmtDate, fmtSecs } from './format';
import { catName, DICTS, lang, locale, matchTag, resolve, setLang, t, tn, tParts } from './i18n.svelte';
import { en, type Msg } from './locales/en';

afterEach(() => setLang('en', 'en-US'));

describe('picking the language', () => {
  // the same cases as the Rust tests in app/src-tauri/src/i18n.rs
  it('matches Simplified Chinese and English only', () => {
    for (const tag of ['zh-CN', 'zh-Hans-CN', 'zh-Hans', 'zh', 'zh-SG', 'zh_CN', 'ZH-hans-HK']) expect(matchTag(tag), tag).toBe('zh-CN');
    for (const tag of ['en', 'en-US', 'en-GB', 'EN-au']) expect(matchTag(tag), tag).toBe('en');
    for (const tag of ['zh-TW', 'zh-HK', 'zh-MO', 'zh-Hant', 'zh-Hant-TW', 'ja-JP', 'fr', '', '-']) expect(matchTag(tag), tag).toBeNull();
  });

  it('takes the first language with a translation', () => {
    expect(resolve(['ja-JP', 'zh-Hans-CN', 'en-US'])).toEqual({ lang: 'zh-CN', locale: 'zh-CN' });
    expect(resolve(['en-GB', 'zh-Hans-CN'])).toEqual({ lang: 'en', locale: 'en-GB' });
    expect(resolve(['zh-Hant-TW', 'en-US']).lang).toBe('en');
  });

  it('falls back to English', () => {
    expect(resolve(['ja-JP', 'ko-KR'])).toEqual({ lang: 'en', locale: 'en-US' });
    expect(resolve([])).toEqual({ lang: 'en', locale: 'en-US' });
    setLang('fr', 'fr-FR');
    expect([lang(), locale()]).toEqual(['en', 'en-US']);
    setLang('zh-CN', 'not a tag!');
    expect([lang(), locale()]).toEqual(['zh-CN', 'zh-CN']);
  });
});

describe('dictionaries', () => {
  const vars = (m: Msg) => [...new Set([...(typeof m === 'string' ? m : `${m.one ?? ''} ${m.other}`).matchAll(/\{(\w+)\}/g)].map(x => x[1]))].sort();

  it('Chinese has every English key, with the same placeholders', () => {
    const zh = DICTS['zh-CN'] as Record<string, Msg>;
    expect(Object.keys(zh).sort()).toEqual(Object.keys(en).sort());
    for (const [k, m] of Object.entries(en)) expect(vars(zh[k]), k).toEqual(vars(m));
  });

  it('shows English for a key a language lacks', () => {
    setLang('zh-CN');
    expect(t('close')).toBe('关闭');
    const zh = DICTS['zh-CN'] as Record<string, Msg | undefined>;
    const saved = zh.close;
    delete zh.close;
    try {
      expect(t('close')).toBe('Close');
    } finally {
      zh.close = saved;
    }
  });

  it('fills placeholders and picks plural forms', () => {
    expect(tn('files', 1)).toBe('1 file');
    expect(tn('files', 12345)).toBe('12,345 files');
    expect(t('toast.recycled', { what: 'a.zip', size: '2 GB' })).toBe('Moved to the Recycle Bin: a.zip (2 GB)');
    setLang('zh-CN');
    expect(tn('files', 1)).toBe('1 个文件');
    expect(tn('selected', 3, { size: '1.2 GB' })).toBe('已选 3 项 · 1.2 GB');
  });

  it('splits text around values for markup', () => {
    expect(tParts('status.used', { used: '480 GB', capacity: '500 GB' })).toEqual([
      { text: 'Used ' },
      { text: '480 GB', v: 'used' },
      { text: ' of ' },
      { text: '500 GB', v: 'capacity' },
    ]);
    setLang('zh-CN');
    expect(tParts('status.free', { free: '20 GB' })).toEqual([{ text: '可用 ' }, { text: '20 GB', v: 'free' }]);
  });

  it('names file categories', () => {
    expect(catName(1)).toBe('Video');
    expect(catName(255)).toBeUndefined();
    setLang('zh-CN');
    expect(catName(10)).toBe('缓存 / 临时');
  });
});

describe('formatting', () => {
  it('keeps the Chinese formats', () => {
    setLang('zh-CN');
    expect(fmtBytes(512)).toBe('512 字节');
    expect(fmtSecs(12_345)).toBe('12.3 秒');
    expect(fmtSecs(125_000)).toBe('2 分 5 秒');
    const d = new Date(2024, 0, 9).getTime();
    expect(fmtDate(d)).toMatch(/^2024\/1\/9（\d+(\.\d)? 年前）$/);
  });

  it('formats English with the user’s locale', () => {
    setLang('en', 'en-GB');
    expect(fmtBytes(1)).toBe('1 byte');
    expect(fmtBytes(512)).toBe('512 bytes');
    expect(fmtSecs(125_000)).toBe('2 min 5 s');
    const now = Date.now();
    expect(fmtDate(now - 3 * 864e5)).toMatch(/^\d\d\/\d\d\/\d{4} \(3 days ago\)$/);
    expect(fmtDate(now - 1.1 * 864e5)).toMatch(/\(1 day ago\)$/);
    expect(fmtDate(now - 400 * 864e5)).toMatch(/\(1\.1 years ago\)$/);
  });
});
