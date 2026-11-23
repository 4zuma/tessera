/**
 * The real backend: Tauri commands and events implemented by the Rust core
 * (crate app/src-tauri). Command names and payloads follow docs/ipc.md.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  Backend,
  BackendEvents,
  CaptionState,
  CatStat,
  DeleteResult,
  Details,
  LayoutRequest,
  NavInfo,
  PlatformInfo,
  ScanStarted,
  SearchResult,
  ShellAction,
  SizeMode,
  Unlisten,
  Volume,
} from './types';

export class TauriBackend implements Backend {
  constructor(readonly platform: PlatformInfo) {}

  listVolumes() {
    return invoke<Volume[]>('list_volumes');
  }
  scanStart(root: string) {
    return invoke<ScanStarted>('scan_start', { root });
  }
  scanCancel() {
    return invoke<void>('scan_cancel');
  }
  layout(req: LayoutRequest) {
    return invoke<ArrayBuffer>('layout', { req });
  }
  details(id: number, view: number, sizeMode: SizeMode) {
    return invoke<Details | null>('details', { id, view, sizeMode });
  }
  nav(id: number) {
    return invoke<NavInfo>('nav', { id });
  }
  catStats(view: number, sizeMode: SizeMode, hide?: number[]) {
    return invoke<CatStat[]>('cat_stats', { view, sizeMode, hide: hide ?? null });
  }
  ageStats(view: number, sizeMode: SizeMode, hide?: number[]) {
    return invoke<number[]>('age_stats', { view, sizeMode, hide: hide ?? null });
  }
  recycle(ids: number[]) {
    return invoke<DeleteResult>('recycle', { ids });
  }
  restore(token: number) {
    return invoke<DeleteResult>('restore', { token });
  }
  purge(ids: number[]) {
    return invoke<DeleteResult>('purge', { ids });
  }
  emptyRecycleBin(root: string) {
    return invoke<DeleteResult>('empty_recycle_bin', { root });
  }
  reveal(id: number) {
    return invoke<void>('reveal', { id });
  }
  shellMenu(ids: number[], x: number, y: number, extended: boolean) {
    return invoke<ShellAction>('shell_menu', { ids, x, y, extended });
  }
  relaunchElevated() {
    return invoke<void>('relaunch_elevated');
  }
  search(query: string, sizeMode: SizeMode) {
    return invoke<SearchResult | null>('search', { query, sizeMode });
  }
  searchClear() {
    return invoke<void>('search_clear');
  }
  caption(action: 'min' | 'max' | 'close') {
    return invoke<void>('caption', { action });
  }
  captionButton(rect: { right: number; top: number; width: number; height: number }) {
    return invoke<void>('caption_button', rect);
  }
  captionState() {
    return invoke<CaptionState>('caption_state');
  }
  loadSettings() {
    return invoke<Record<string, number | string>>('settings_load');
  }
  saveSetting(name: string, value: number | string | null) {
    return invoke<void>('settings_save', { name, value });
  }
  clearSettings() {
    return invoke<void>('settings_clear');
  }
  on<K extends keyof BackendEvents>(event: K, cb: (payload: BackendEvents[K]) => void): Promise<Unlisten> {
    return listen<BackendEvents[K]>(event, e => cb(e.payload));
  }
}
