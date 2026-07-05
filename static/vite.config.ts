/// <reference types="vitest/config" />
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    globals: true,
    // The jsdom + MSW + userEvent component tests (e.g. SequenceView) can take
    // several seconds on a loaded/slow CI runner — the whole suite ran ~15 s for
    // 19 tests on Windows CI. Vitest's 5 s default flakes there, so raise the
    // ceiling. This still fails a genuinely hung test, just not a slow-but-fine one.
    testTimeout: 20_000,
    // Don't let vitest scan the Playwright suite — those specs use
    // `@playwright/test` and can't run under vitest.
    exclude: ['**/node_modules/**', '**/dist/**', 'e2e/**'],
  },
  server: {
    proxy: {
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      },
    },
  },
  build: {
    // Output to dist directory
    outDir: 'dist',
    // Generate source maps for debugging
    sourcemap: true,
    // Optimize chunk size
    rollupOptions: {
      output: {
        manualChunks: (id) => {
          if (id.includes('node_modules/react/') || id.includes('node_modules/react-dom/')) {
            return 'vendor'
          }
          if (id.includes('node_modules/@tanstack/react-query/') || id.includes('node_modules/axios/')) {
            return 'query'
          }
        },
      },
    },
  },
  base: '/',
})
