import { expect, test } from '@playwright/test';
import { openSettings, registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// Settings › Run a worker in the desktop app. The page runs in a browser
// with a stand-in for the desktop app's commands: Rust tests cover the
// settings file, the start-at-login entries and the worker's controls, and
// a real worker runs in the menu bar in manual testing.

test.beforeEach(async ({ page, request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request);
  await waitForCacheReady(request, entry.id);
  await page.addInitScript(() => {
    type Worker = Record<string, unknown> & { status: unknown; settings: unknown; has_token: boolean };
    const worker: Worker = {
      supported: true,
      settings: null,
      has_token: false,
      start_at_login: false,
      status: null,
      settings_path: '/Users/ann/Library/Application Support/psf-guard/worker.json',
    };
    const calls: Array<{ command: string; args: unknown }> = [];
    (window as unknown as { __desktopWorkerCalls: typeof calls }).__desktopWorkerCalls = calls;
    const origin = window.location.origin;
    const commands: Record<string, (args: Record<string, unknown>) => unknown> = {
      get_server_url: () => origin,
      get_desktop_server_settings: () => ({
        server_port: null, active_server_port: Number(window.location.port), server_url: origin,
        starting: false, error: null,
      }),
      is_configuration_valid: () => true,
      get_log_folder: () => '/Users/ann/Library/Logs/com.theatrus.psf-guard',
      get_desktop_worker: () => worker,
      save_desktop_worker: ({ worker: saved }) => {
        const { token, start_at_login, ...settings } = saved as Record<string, unknown>;
        worker.settings = settings;
        worker.has_token = worker.has_token || Boolean(token);
        worker.start_at_login = start_at_login;
        return worker;
      },
      start_desktop_worker: () => {
        worker.status = {
          server: 'http://observatory.local:3000', name: 'studio-mac', version: '0.13.0',
          connection: 'connected', paused: false, problem: null,
          tasks: [{ id: 'a', label: 'Stack of 42 frames, Ha channel', fraction: 0.37 }],
        };
        return worker;
      },
      stop_desktop_worker: () => {
        worker.status = null;
        return worker;
      },
    };
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: async (command: string, args: Record<string, unknown> = {}) => {
        calls.push({ command, args });
        const run = commands[command];
        return run ? run(args) : null;
      },
      transformCallback: () => 0,
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main', windowLabel: 'main' } },
    };
  });
});

test('this computer saves its worker settings, then starts and stops the worker', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 1400 });
  await page.goto('/');
  await openSettings(page);
  await page.getByRole('tab', { name: 'Run a worker' }).click();
  const start = page.getByRole('button', { name: 'Start the worker' });
  await expect(start).toBeDisabled();

  await page.getByLabel('Server address').fill('http://observatory.local:3000');
  await page.getByLabel('Worker token').fill(`psfw_${'a'.repeat(64)}`);
  await page.getByLabel('Worker name').fill('studio-mac');
  await page.getByRole('checkbox', { name: /RC-Astro tools/ }).uncheck();
  await page.getByRole('radio', { name: /shares with the server/ }).check();
  await page.getByLabel('Server folder 1').fill('/srv/astro');
  await page.getByLabel("This computer's folder 1").fill('/Volumes/astro');
  await page.getByRole('checkbox', { name: 'Start the worker when I log in' }).check();
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Saved.' })).toBeVisible();
  const saved = await page.evaluate(() =>
    (window as unknown as { __desktopWorkerCalls: Array<{ command: string; args: { worker?: unknown } }> })
      .__desktopWorkerCalls.find((call) => call.command === 'save_desktop_worker')?.args.worker);
  expect(saved).toEqual({
    server: 'http://observatory.local:3000',
    name: 'studio-mac',
    files: 'shared',
    maps: [{ server: '/srv/astro', local: '/Volumes/astro' }],
    work_dir: null,
    rc_astro: null,
    work: ['stack'],
    slots: 1,
    token: `psfw_${'a'.repeat(64)}`,
    start_at_login: true,
  });

  await start.click();
  const status = page.getByRole('region', { name: 'Worker status' });
  await expect(status).toContainText('Connected to http://observatory.local:3000 as studio-mac');
  await expect(status).toContainText('Stack of 42 frames, Ha channel · 37%');
  await page.locator('.desktop-worker-settings').screenshot({ path: testInfo.outputPath('run-a-worker.png') });
  await status.getByRole('button', { name: 'Stop' }).click();
  await expect(status).toContainText('Not running.');
});
