import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Tauri serves the built files from disk; relative paths also let the same build
// be opened as a static preview.
export default defineConfig({
  base: './',
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: 'chrome120', // WebView2 on Windows 10/11 is evergreen Chromium
    cssMinify: true,
    sourcemap: false,
    reportCompressedSize: false,
  },
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
});
