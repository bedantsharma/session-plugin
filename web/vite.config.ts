/// <reference types="vitest/config" />
import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import { svelteTesting } from '@testing-library/svelte/vite'
import path from 'node:path'
import { defineConfig } from 'vite'

// The dashboard. The desktop card is a second, single-file build: vite.card.config.ts.
export default defineConfig({
  plugins: [tailwindcss(), svelte(), svelteTesting()],
  resolve: {
    alias: { $lib: path.resolve('./src/lib') },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  test: {
    environment: 'jsdom',
    include: ['tests/**/*.test.ts'],
    setupFiles: ['tests/setup.ts'],
  },
})
