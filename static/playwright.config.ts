import { defineConfig, devices } from '@playwright/test';
import * as os from 'os';
import * as path from 'path';
import { installAstrometryFixture } from './e2e/fixtures/astrometry';
import { installSyncFixture } from './e2e/fixtures/sync';

const PORT = Number(process.env.PSF_GUARD_E2E_PORT ?? 13099);
// The sync pair: two more `psf-guard server` processes, each with its own
// registry, cache, and catalog, standing in for two machines syncing with
// each other.
const TELESCOPE_PORT = PORT + 1;
const REVIEW_PORT = PORT + 2;

// Per-run tmp directory for the registry, cache, and fixture SQLite. The
// global setup wipes and recreates this; the `webServer` entries below point
// each `psf-guard server` instance at it via --registry / --cache-dir so the
// test run never touches the user's real config.
//
// Playwright re-evaluates this config in every worker process, where
// `process.pid` is the worker's. Honour the variable global setup exports so
// workers resolve the same directory the servers were started against
// instead of inventing an empty one of their own.
const TMP_BASE =
  process.env.PSF_GUARD_E2E_TMP ??
  path.join(os.tmpdir(), `psf-guard-e2e-${process.pid}`);

// `webServer` starts before Playwright's global setup. Seed the process-global
// astrometry registry while evaluating the config so the Rust server sees it
// during startup; global setup restores the same fixture after its reset.
installAstrometryFixture(TMP_BASE);

// One scheduler catalog and one config per sync instance. Seeded here, not in
// global setup, because each server reads its registry and config at startup.
const syncFixture = installSyncFixture(TMP_BASE);
// Specs read these from the runner's own environment; webServer `env` only
// reaches the server child processes.
process.env.PSF_GUARD_E2E_TELESCOPE_URL = `http://127.0.0.1:${TELESCOPE_PORT}`;
process.env.PSF_GUARD_E2E_REVIEW_URL = `http://127.0.0.1:${REVIEW_PORT}`;
process.env.PSF_GUARD_E2E_SYNC_UPLOAD_DIR = syncFixture.uploadDir;
process.env.PSF_GUARD_E2E_REVIEW_CACHE = syncFixture.review.cacheDir;

const serverCommand =
  process.env.PSF_GUARD_E2E_BINARY ??
  'cd .. && cargo run --release --bin psf-guard --';

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

  webServer: [
    {
      // Run the CLI server against an isolated registry and cache directory.
      // --allow-database-management lets the e2e specs exercise the CRUD UI.
      //
      // PSF_GUARD_E2E_BINARY (set in CI) skips the cargo build and points
      // straight at a prebuilt binary. Locally, leave it unset and we'll
      // `cargo run --release` from the repo root.
      command:
        `${serverCommand} server ` +
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
    // The two ends of a sync. Neither takes --allow-database-management: a
    // server that accepts remote sync must not need the CRUD grant to do it,
    // and that is part of what these specs check. They are also kept off the
    // main instance because the CRUD specs reset its database list.
    ...[
      { port: TELESCOPE_PORT, instance: syncFixture.telescope },
      { port: REVIEW_PORT, instance: syncFixture.review },
    ].map(({ port, instance }) => ({
      command:
        `${serverCommand} server ` +
        `--port ${port} ` +
        `--registry ${instance.registryPath} ` +
        `--cache-dir ${instance.cacheDir} ` +
        `--config ${instance.configPath}`,
      url: `http://127.0.0.1:${port}/api/info`,
      timeout: 180_000,
      reuseExistingServer: !process.env.CI,
      env: {
        ...(dyldFallback ? { DYLD_FALLBACK_LIBRARY_PATH: dyldFallback } : {}),
        RUST_LOG: 'info',
      },
    })),
  ],
});
