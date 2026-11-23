/**
 * The design-system bundle: one classic script (IIFE) that sets window.Tessera, plus its
 * stylesheet. React comes from the page (window.React), so it is not bundled. The app's
 * backend picker (src/lib/backend/index.ts) is swapped for design/src/backend.ts: always the
 * mock, never Tauri.
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

const here = path.dirname(fileURLToPath(import.meta.url));
const appBackend = path.resolve(here, '../src/lib/backend/index.ts');
const designBackend = path.resolve(here, 'src/backend.ts');

/** @returns {import('vite').Plugin} */
function swapBackend() {
  return {
    name: 'tessera-design-backend',
    enforce: 'pre',
    async resolveId(source, importer, options) {
      if (!importer || importer === designBackend || !source.endsWith('backend')) return null;
      const r = await this.resolve(source, importer, { ...options, skipSelf: true });
      return r && path.resolve(r.id) === appBackend ? designBackend : null;
    },
  };
}

export default defineConfig({
  root: here,
  base: './',
  plugins: [svelte({ configFile: path.resolve(here, '../svelte.config.js') }), swapBackend()],
  define: { 'process.env.NODE_ENV': '"production"' },
  logLevel: 'warn',
  build: {
    outDir: path.resolve(here, 'out/bundle'),
    emptyOutDir: true,
    target: 'chrome120',
    cssCodeSplit: false,
    minify: true,
    sourcemap: false,
    reportCompressedSize: false,
    lib: {
      entry: path.resolve(here, 'src/index.ts'),
      name: 'Tessera',
      formats: ['iife'],
      fileName: () => 'bundle.js',
      cssFileName: 'bundle',
    },
  },
});
