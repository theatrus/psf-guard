import { defineConfig, devices } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

const root = process.env.PSF_GUARD_DIRECTOR_E2E_TMP ?? mkdtempSync(path.join(tmpdir(), 'psf-guard-director-ui-'));
process.env.PSF_GUARD_DIRECTOR_E2E_TMP = root;
const port = Number(process.env.PSF_GUARD_DIRECTOR_E2E_PORT ?? 13742);
const binary = process.env.PSF_GUARD_E2E_BINARY;
const command = binary ? `"${binary}"` : 'cargo run --manifest-path ../Cargo.toml --bin psf-guard --';

export default defineConfig({
  testDir: './e2e-director',
  workers: 1,
  retries: 0,
  timeout: 30_000,
  use: { baseURL: `http://127.0.0.1:${port}`, trace: 'retain-on-failure', screenshot: 'only-on-failure' },
  projects: [{ name: 'chromium', use: devices['Desktop Chrome'] }],
  webServer: {
    command: `${command} server --host 127.0.0.1 --port ${port} --registry "${path.join(root, 'registry.json')}" --cache-dir "${path.join(root, 'cache')}" --director-meta "${path.join(root, 'meta.sqlite')}" --allow-database-management --static-dir "${path.resolve('dist')}"`,
    url: `http://127.0.0.1:${port}/api/info`,
    reuseExistingServer: false,
    timeout: 180_000,
  },
});
