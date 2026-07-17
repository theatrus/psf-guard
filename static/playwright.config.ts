import { defineConfig, devices } from '@playwright/test';
import * as os from 'os';
import * as path from 'path';
import { installAstrometryFixture } from './e2e/fixtures/astrometry';

const PORT = Number(process.env.PSF_GUARD_E2E_PORT ?? 13099);

// Per-process tmp directory for the registry, cache, and fixture SQLite. The
// global setup wipes and recreates this; `webServer` below points the
// `psf-guard server` instance at it via --registry / --cache-dir so the test
// run never touches the user's real config.
const TMP_BASE = path.join(os.tmpdir(), `psf-guard-e2e-${process.pid}`);

// `webServer` starts before Playwright's global setup. Seed the process-global
// astrometry registry while evaluating the config so the Rust server sees it
// during startup; global setup restores the same fixture after its reset.
installAstrometryFixture(TMP_BASE);

// macOS local dev needs OpenCV's libclang.dylib reachable; CI / Linux usually
// doesn't. Pass through whatever the parent shell has set; if nothing's set
// and we're on macOS, fall back to the Command Line Tools default path.
const dyldFallback =
  process.env.DYLD_FALLBACK_LIBRARY_PATH ??
  (process.platform === 'darwin'
    ? '/Library/Developer/CommandLineTools/usr/lib'
    : undefined);

export default defineConfig({
  testDir: './e2e',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 30_000,
  reporter: process.env.CI ? [['list'], ['github']] : 'list',

  globalSetup: './e2e/global-setup.ts',

  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    actionTimeout: 5_000,
    navigationTimeout: 10_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    // `video: 'retain-on-failure'` would also help debug interaction races,
    // but the trace.zip already covers that and videos balloon CI artifacts.
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],

  webServer: {
    // Run the CLI server against an isolated registry and cache directory.
    // --allow-database-management lets the e2e specs exercise the CRUD UI.
    //
    // PSF_GUARD_E2E_BINARY (set in CI) skips the cargo build and points
    // straight at a prebuilt binary. Locally, leave it unset and we'll
    // `cargo run --release` from the repo root.
    command:
      `${process.env.PSF_GUARD_E2E_BINARY ?? 'cd .. && cargo run --release --bin psf-guard --'} ` +
      `server ` +
      `--port ${PORT} ` +
      `--registry ${path.join(TMP_BASE, 'registry.json')} ` +
      `--cache-dir ${path.join(TMP_BASE, 'cache')} ` +
      `--allow-database-management`,
    url: `http://127.0.0.1:${PORT}/api/info`,
    timeout: 180_000,
    reuseExistingServer: !process.env.CI,
    env: {
      ...(dyldFallback ? { DYLD_FALLBACK_LIBRARY_PATH: dyldFallback } : {}),
      // Expose the tmp base to specs so they can reach the fixture files.
      PSF_GUARD_E2E_TMP: TMP_BASE,
      RUST_LOG: 'info',
    },
  },
});
