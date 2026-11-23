/**
 * The view options Tessera remembers between runs. Rust keeps them in the registry under
 * HKEY_CURRENT_USER\Software\Tessera (app/src-tauri/src/settings.rs), next to the window's
 * place and size, which Rust saves itself. Only what differs from the default is stored:
 * saving a default value deletes it, so the key stays empty, and then goes away, for
 * anyone who keeps the defaults.
 */
import type { SizeMode } from './backend/types';

export interface Prefs {
  /** what tiles are sized by */
  sizeMode: SizeMode;
  /** the details pane is shown (when the window is wide enough for it) */
  pane: boolean;
}

export const DEFAULTS: Prefs = { sizeMode: 'disk', pane: true };

/** what the registry holds, by value name: numbers are REG_DWORD, text REG_SZ */
export type Stored = Record<string, number | string>;

/** The value name and what to store for one setting; `null` deletes the value. */
export function toStored<K extends keyof Prefs>(key: K, v: Prefs[K]): [string, number | string | null] {
  switch (key) {
    case 'sizeMode':
      return ['SizeMode', v === DEFAULTS.sizeMode ? null : (v as SizeMode)];
    case 'pane':
      return ['DetailsPane', v === DEFAULTS.pane ? null : v ? 1 : 0];
  }
  throw new Error(`unknown setting ${key}`);
}

/** The settings in `s`; anything missing or not understood is the default. */
export function fromStored(s: Stored): Prefs {
  const p = { ...DEFAULTS };
  if (s.SizeMode === 'logical' || s.SizeMode === 'disk') p.sizeMode = s.SizeMode;
  if (s.DetailsPane === 0 || s.DetailsPane === 1) p.pane = s.DetailsPane === 1;
  return p;
}
