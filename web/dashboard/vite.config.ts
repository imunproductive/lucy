import {defineConfig} from 'vite';
import {svelte} from '@sveltejs/vite-plugin-svelte';

// `npm run dev` serves the page with hot reload and sends the API and the
// pictures to a running `lucy serve` (its dashboard port, 3725 unless
// LUCY_DASHBOARD names another address).
const server = process.env.LUCY_DASHBOARD ?? 'http://127.0.0.1:3725';
const proxy = {target: server, changeOrigin: true};

export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    emptyOutDir: true,
    target: 'es2022',
    modulePreload: {polyfill: false},
    reportCompressedSize: false,
  },
  server: {
    proxy: {
      '/api': proxy,
      '/assets/boxes': proxy,
      '/assets/previews': proxy,
    },
  },
});
