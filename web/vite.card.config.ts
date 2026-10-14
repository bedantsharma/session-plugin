import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import path from 'node:path'
import { defineConfig } from 'vite'
import { viteSingleFile } from 'vite-plugin-singlefile'

// The desktop card (MCP Apps, spec §10.5): one self-contained HTML file, embedded in the
// binary and served as the ui://bridge-screen/progress resource.
export default defineConfig({
  plugins: [tailwindcss(), svelte(), viteSingleFile()],
  resolve: {
    alias: { $lib: path.resolve('./src/lib') },
  },
  build: {
    outDir: 'dist-card',
    emptyOutDir: true,
    rollupOptions: { input: 'card.html' },
  },
})
