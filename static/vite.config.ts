/// <reference types="vitest/config" />
import { createHash } from 'node:crypto'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'

/** Stamps index.html with the build it belongs to: a hash of the page and
 *  of every file the build wrote, all named by their content. The server
 *  reports the same stamp on each reply, so an open tab can tell when the
 *  server holds a newer page (`src/updates/pageBuild.ts`). */
const buildStamp = (): Plugin => ({
  name: 'psf-guard-build-stamp',
  apply: 'build',
  transformIndexHtml: {
    order: 'post',
    handler: (html, { bundle }) => {
      const files = Object.keys(bundle ?? {}).sort().join('\n')
      const build = createHash('sha256').update(html).update(files).digest('hex').slice(0, 16)
      return [{ tag: 'meta', attrs: { name: 'psf-guard-build', content: build }, injectTo: 'head' }]
    },
  },
})

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), buildStamp()],
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
    exclude: ['**/node_modules/**', '**/dist/**', 'e2e/**', 'e2e-director/**'],
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
