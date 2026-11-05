/**
 * The real backend: Tauri commands and events implemented by the Rust core
 * (crate app/src-tauri). Command names and payloads follow docs/ipc.md.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  Backend,
  BackendEvents,
  CatStat,
  DeleteResult,
  Details,
  LayoutRequest,
  NavInfo,
  PlatformInfo,
  ScanStarted,
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
  catStats(view: number, sizeMode: SizeMode) {
    return invoke<CatStat[]>('cat_stats', { view, sizeMode });
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
  relaunchElevated() {
    return invoke<void>('relaunch_elevated');
  }
  on<K extends keyof BackendEvents>(event: K, cb: (payload: BackendEvents[K]) => void): Promise<Unlisten> {
    return listen<BackendEvents[K]>(event, e => cb(e.payload));
  }
}
