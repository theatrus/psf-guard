import { expect, test, type Page } from '@playwright/test';
import { openSettings, registerFixtureDb } from './helpers';

// The native lifecycle is tested against real sockets in tauri_main. These
// browser tests exercise the desktop bridge, settings, and recovery UI while
// the isolated fixture server supplies the HTTP API.
async function desktopBridge(page: Page, conflict = false) {
  await page.addInitScript(({ conflict }) => {
    const key = 'desktop-server-e2e';
    const saved = localStorage.getItem(key);
    const preference: { port: number | null; active: number | null; conflict: boolean } = saved
      ? JSON.parse(saved) : { port: conflict ? 3030 : null, active: null, conflict };
    const status = () => ({
      server_port: preference.port,
      active_server_port: preference.active,
      server_url: preference.conflict ? null : location.origin,
      starting: false,
      error: preference.conflict ? 'Could not start the desktop server on 127.0.0.1:3030. The port is already in use.' : null,
    });
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {
      invoke: async (command: string, args?: { port?: number | null }) => {
        switch (command) {
          case 'get_desktop_server_settings': return status();
          case 'get_server_url': {
            if (preference.conflict) throw new Error('The chosen port is occupied');
            return location.origin;
          }
          case 'set_desktop_server_port':
            preference.port = args?.port ?? null;
            localStorage.setItem(key, JSON.stringify(preference));
            return status();
          case 'restart_server':
            preference.active = preference.port;
            preference.conflict = false;
            localStorage.setItem(key, JSON.stringify(preference));
            return location.origin;
          case 'is_configuration_valid': return true;
          case 'get_current_configuration': return { schema_version: 2, databases: [] };
          case 'get_log_folder': return null;
          case 'plugin:updater|check': return null;
          default: throw new Error(`Unmocked desktop command: ${command}`);
        }
      },
    } });
  }, { conflict });
}

async function openServerSettings(page: Page) {
  await openSettings(page);
  await page.getByRole('tab', { name: 'Server', exact: true }).click();
  return page.getByRole('region', { name: 'Desktop server' });
}

test('desktop Server settings retain a fixed port through restart and show the same agent address', async ({ page, baseURL }, testInfo) => {
  await desktopBridge(page);
  await page.goto('/');
  const server = await openServerSettings(page);
  await server.getByRole('radio', { name: 'Fixed port' }).check();
  const port = Number(new URL(baseURL!).port);
  await server.getByRole('spinbutton', { name: 'Server port' }).fill(String(port));
  await server.getByRole('button', { name: 'Save port' }).click();
  await expect(server.getByRole('status')).toContainText(`Port ${port} is saved`);
  await Promise.all([
    page.waitForEvent('load'),
    server.getByRole('button', { name: 'Restart server', exact: true }).click(),
  ]);
  const restored = await openServerSettings(page);
  await expect(restored.getByRole('radio', { name: 'Fixed port' })).toBeChecked();
  await expect(restored.getByRole('spinbutton')).toHaveValue(String(port));
  await expect(restored).toContainText(baseURL!);
  await page.screenshot({ path: testInfo.outputPath('server-settings.png'), fullPage: true });
  await page.getByRole('tab', { name: 'Agents', exact: true }).click();
  await expect(page.getByText(`claude mcp add --transport http psf-guard ${baseURL}/api/mcp`, { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Choose a fixed server port' }).click();
  await expect(page.getByRole('tab', { name: 'Server', exact: true })).toHaveAttribute('aria-selected', 'true');
});

test('desktop startup can recover from a port conflict without contacting the conflicting server', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await desktopBridge(page, true);
  const apiRequests: string[] = [];
  page.on('request', request => {
    if (new URL(request.url()).pathname.startsWith('/api/')) apiRequests.push(request.url());
  });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Desktop server settings' })).toBeVisible();
  await expect(page.getByRole('alert')).toContainText('127.0.0.1:3030');
  expect(apiRequests).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath('startup-recovery.png'), fullPage: true });
  const server = page.getByRole('region', { name: 'Desktop server' });
  await server.getByRole('radio', { name: 'Automatic' }).check();
  await server.getByRole('button', { name: 'Save port' }).click();
  await expect(server.getByRole('status')).toContainText('Automatic port selection is saved');
  await Promise.all([
    page.waitForEvent('load'),
    server.getByRole('button', { name: 'Restart server', exact: true }).click(),
  ]);
  await expect(page.getByRole('button', { name: 'Menu', exact: true })).toBeVisible();
  const restored = await openServerSettings(page);
  await expect(restored.getByRole('radio', { name: 'Automatic' })).toBeChecked();
});

test('browser Settings leave the listener choice to the server operator', async ({ page, request }) => {
  await registerFixtureDb(request, { slug: 'desktop-port-browser' });
  await page.goto('/');
  await openSettings(page);
  await expect(page.getByRole('tab', { name: 'Agents', exact: true })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Server', exact: true })).toHaveCount(0);
});
