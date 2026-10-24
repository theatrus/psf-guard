import { defineConfig, devices } from '@playwright/test';
import releaseHighlights from './src/updates/highlights.json' with { type: 'json' };
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

// Locally, end a hung run instead of letting it hold browsers forever.
// scripts/e2e-local.mjs sets this just under its own hard limit. CI keeps
// Playwright's default of no limit; the job has its own timeout.
const globalTimeout = process.env.CI
  ? 0
  : Number(process.env.PSF_GUARD_E2E_GLOBAL_TIMEOUT_MS ?? 30 * 60_000);

// Reusing a server already on the port is handy by hand, but under
// scripts/e2e-local.mjs that server would belong to another run.
const reuseExistingServer = !process.env.CI && !process.env.PSF_GUARD_E2E_NO_REUSE;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 30_000,
  globalTimeout,
  reporter: process.env.CI ? [['list'], ['github']] : 'list',

  globalSetup: './e2e/global-setup.ts',
  // Removes this run's tmp directories, ~1.8 GB of copied FITS fixtures.
  // Set PSF_GUARD_E2E_KEEP_TMP to keep them while debugging a failure.
  globalTeardown: './e2e/global-teardown.ts',

  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    // The Library opens compact by default; these specs read the full
    // project cards, so every browser starts with the detailed view set,
    // the way a user who chose it under Settings would. Compact rows have
    // their own unit coverage.
    storageState: {
      cookies: [],
      origins: [PORT, TELESCOPE_PORT, REVIEW_PORT].map((port) => ({
        origin: `http://127.0.0.1:${port}`,
        localStorage: [
          // Most specs test the app after its release notes were acknowledged.
          { name: 'psf-guard.whats-new.acknowledged', value: releaseHighlights[0].version },
          { name: 'psf-guard.display-preferences', value: JSON.stringify({ libraryDensity: 'detailed' }) },
        ],
      })),
    },
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
    // Safari's engine, which the macOS app's WebView and every iPhone
    // browser share. It has no CSS scroll anchoring, lays out phones its
    // own way and draws native form fields itself, so the detail's scroll
    // and phone specs, the View choices' long-page flip, form fields and
    // compact job-queue controls and the Sequence chart's dock run here
    // too; the rest stay Chromium-only.
    //
    // It asks for reduced motion. CI's WebKit draws in software, and while
    // a dozen preview spinners turn it can go seconds without drawing a
    // frame; an action that waits for an element to hold still across two
    // frames then times out. Spinners hold still under reduced motion.
    {
      name: 'webkit',
      use: { ...devices['Desktop Safari'], contextOptions: { reducedMotion: 'reduce' } },
      testMatch: ['wbpp-storage.spec.ts', 'detail-scroll.spec.ts', 'detail-phone.spec.ts', 'view-choices.spec.ts', 'form-fields.spec.ts', 'astrobin-export.spec.ts', 'whats-new.spec.ts', 'server-info.spec.ts', 'auth.spec.ts', 'desktop-zoom.spec.ts', 'job-queue.spec.ts', 'sequence-dock.spec.ts', 'stack-method-settings.spec.ts', 'project-lifecycle.spec.ts', 'settings-scroll.spec.ts'],
    },
  ],

  // Playwright starts each server as its own process group and kills that
  // group when the run ends, even on failure or Ctrl-C. Only a runner killed
  // with SIGKILL leaves them behind; scripts/e2e-local.mjs then stops the
  // groups itself.
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
        // Loopback, because database management with no user account is a
        // localhost-only grant. The specs reach the server here anyway.
        `--host 127.0.0.1 ` +
        `--registry ${path.join(TMP_BASE, 'registry.json')} ` +
        `--cache-dir ${path.join(TMP_BASE, 'cache')} ` +
        `--allow-database-management`,
      url: `http://127.0.0.1:${PORT}/api/info`,
      timeout: 180_000,
      reuseExistingServer,
      env: {
        ...(dyldFallback ? { DYLD_FALLBACK_LIBRARY_PATH: dyldFallback } : {}),
        // Expose the tmp base to specs so they can reach the fixture files.
        PSF_GUARD_E2E_TMP: TMP_BASE,
        RUST_LOG: 'info',
      },
    },
    // The two ends of a sync, kept off the main instance because the CRUD
    // specs reset its database list.
    //
    // Only the review copy — the machine someone sits at — takes
    // --allow-database-management, because configuring a peer is registry
    // work. The telescope has none, which is how the specs show that
    // *accepting* a sync never needs that grant.
    ...[
      { port: TELESCOPE_PORT, instance: syncFixture.telescope, manage: false },
      { port: REVIEW_PORT, instance: syncFixture.review, manage: true },
    ].map(({ port, instance, manage }) => ({
      command:
        `${serverCommand} server ` +
        `--port ${port} ` +
        `--host 127.0.0.1 ` +
        `--registry ${instance.registryPath} ` +
        `--cache-dir ${instance.cacheDir} ` +
        `--config ${instance.configPath}` +
        (manage ? ' --allow-database-management' : ''),
      url: `http://127.0.0.1:${port}/api/info`,
      timeout: 180_000,
      reuseExistingServer,
      env: {
        ...(dyldFallback ? { DYLD_FALLBACK_LIBRARY_PATH: dyldFallback } : {}),
        RUST_LOG: 'info',
      },
    })),
  ],
});
