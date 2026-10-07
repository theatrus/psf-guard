import { defineConfig, devices } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';

const root = process.env.PSF_GUARD_DIRECTOR_E2E_TMP ?? mkdtempSync(path.join(tmpdir(), 'psf-guard-director-ui-'));
// Some Windows test hosts grant extra users access to TEMP. Protect only this
// newly created fixture directory, keeping the production credential check intact.
if (!process.env.PSF_GUARD_DIRECTOR_E2E_TMP && process.platform === 'win32') {
  execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', `
    $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = New-Object System.Security.AccessControl.DirectorySecurity
    $acl.SetOwner($sid)
    $acl.SetAccessRuleProtection($true, $false)
    $rule = New-Object System.Security.AccessControl.FileSystemAccessRule($sid, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
    $acl.AddAccessRule($rule)
    [System.IO.Directory]::SetAccessControl($env:PSF_GUARD_PRIVATE_TEST_ROOT, $acl)
  `], { env: { ...process.env, PSF_GUARD_PRIVATE_TEST_ROOT: root }, windowsHide: true });
}
process.env.PSF_GUARD_DIRECTOR_E2E_TMP = root;
const port = Number(process.env.PSF_GUARD_DIRECTOR_E2E_PORT ?? 13742);
const binary = process.env.PSF_GUARD_E2E_BINARY;
const command = binary ? `"${binary}"` : 'cargo run --manifest-path ../Cargo.toml --bin psf-guard --';

export default defineConfig({
  testDir: './e2e-director',
  workers: 1,
  retries: 0,
  timeout: 30_000,
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    // The plan list opens compact by default; these specs read the full cards.
    storageState: {
      cookies: [],
      origins: [{ origin: `http://127.0.0.1:${port}`, localStorage: [{ name: 'psf-guard.display-preferences', value: JSON.stringify({ libraryDensity: 'detailed' }) }] }],
    },
  },
  projects: [{ name: 'chromium', use: devices['Desktop Chrome'] }],
  webServer: {
    command: `${command} server --host 127.0.0.1 --port ${port} --registry "${path.join(root, 'registry.json')}" --cache-dir "${path.join(root, 'cache')}" --director-meta "${path.join(root, 'meta.sqlite')}" --allow-database-management --static-dir "${path.resolve('dist')}"`,
    url: `http://127.0.0.1:${port}/api/info`,
    reuseExistingServer: false,
    timeout: 180_000,
  },
});
