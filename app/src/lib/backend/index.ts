import { invoke } from '@tauri-apps/api/core';
import { MockBackend } from './mock';
import { TauriBackend } from './tauri';
import type { Backend, PlatformInfo } from './types';

export const inTauri = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/**
 * Inside the Tauri window, ask Rust what it supports. Until the scan core is wired in
 * (`core: false`) the tree comes from the mock, while the window itself (Mica, accent
 * colour) is already real. In a plain browser everything is mocked.
 */
export async function createBackend(): Promise<Backend> {
  if (inTauri()) {
    try {
      const info = { ...(await invoke<PlatformInfo>('platform_info')), native: true };
      return info.core ? new TauriBackend(info) : new MockBackend(info);
    } catch (e) {
      console.warn('platform_info failed, using the mock backend', e);
      return new MockBackend({ native: true });
    }
  }
  return new MockBackend();
}

export * from './types';
