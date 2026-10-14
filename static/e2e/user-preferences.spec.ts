import { expect, test, type Browser, type Page } from '@playwright/test';
import { spawn, spawnSync, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { fixtureDbPath, fixtureImageDir, tmpBase } from './helpers';

// A signed-in user's View choices follow them to a fresh browser, and stay
// theirs. These specs need user accounts, which the shared servers do not
// have, so they start one of their own on the fixture catalog.

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BINARY = process.env.PSF_GUARD_E2E_BINARY
  ?? path.resolve(__dirname, '../../target/release/psf-guard');
const PORT = Number(process.env.PSF_GUARD_E2E_PORT ?? 13099) + 7;
const BASE = `http://127.0.0.1:${PORT}`;
const PASSWORD = 'a-long-e2e-password';

let server: ChildProcess | undefined;
let dbId = '';

test.describe.configure({ mode: 'serial' });

test.beforeAll(async () => {
  const directory = path.join(tmpBase(), `user-preferences-${process.pid}`);
  fs.rmSync(directory, { recursive: true, force: true });
  fs.mkdirSync(directory, { recursive: true });
  const catalog = path.join(directory, 'scheduler.sqlite');
  fs.copyFileSync(fixtureDbPath(), catalog);
  const registry = path.join(directory, 'registry.json');
  const passwordFile = path.join(directory, 'password');
  fs.writeFileSync(passwordFile, `${PASSWORD}\n`);
  // A viewer, to show a read-only account saves its own view, and an editor.
  for (const [name, role] of [['alice', 'read-only'], ['bob', 'read-write']]) {
    const added = spawnSync(BINARY, [
      'users', 'add', name, '--role', role, '--password-file', passwordFile, '--registry', registry,
    ], { encoding: 'utf8' });
    expect(added.status, added.stderr).toBe(0);
  }
  server = spawn(BINARY, [
    'server', catalog, fixtureImageDir(),
    '--registry', registry,
    '--cache-dir', path.join(directory, 'cache'),
    '--host', '127.0.0.1',
    '--port', String(PORT),
  ], { stdio: 'ignore' });

  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      const response = await fetch(`${BASE}/api/auth/status`);
      if (response.ok) break;
    } catch {
      // Not listening yet.
    }
    if (Date.now() > deadline) throw new Error('The accounts server did not start');
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
});

test.afterAll(() => {
  server?.kill();
});

/** A browser with nothing of its own yet, signed in as `name`. */
async function signIn(browser: Browser, name: string): Promise<Page> {
  const context = await browser.newContext({ storageState: { cookies: [], origins: [] } });
  const page = await context.newPage();
  await page.goto(`${BASE}/`);
  await page.getByLabel('Username').fill(name);
  await page.getByLabel('Password').fill(PASSWORD);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to PSF Guard' })).toHaveCount(0);
  if (!dbId) {
    dbId = await page.evaluate(async () => {
      const response = await fetch('/api/databases');
      const body = await response.json();
      return body.data[0].id as string;
    });
  }
  return page;
}

async function openGrid(page: Page) {
  // The link names no order and no grouping: the saved ones apply.
  await page.goto(`${BASE}/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card')).toHaveCount(3, { timeout: 30_000 });
  await page.getByRole('button', { name: /^View/ }).click();
}

test('a user\'s order and grouping come back in a fresh browser, and stay theirs', async ({ browser }) => {
  const first = await signIn(browser, 'alice');
  await openGrid(first);
  await expect(first.getByLabel('Group', { exact: true })).toHaveValue('filter');
  const saved = first.waitForResponse((response) =>
    response.url().endsWith('/api/me/preferences')
    && response.request().method() === 'PUT'
    && response.ok()
    && (response.request().postData() ?? '').includes('"imageGrouping":"session"')
    && (response.request().postData() ?? '').includes('"imageOrder":"newest"'));
  await first.getByRole('radio', { name: 'Newest first' }).click();
  await first.getByLabel('Group', { exact: true }).selectOption('session');
  await saved;
  await first.context().close();

  // Another browser: the same person finds the same view.
  const again = await signIn(browser, 'alice');
  await openGrid(again);
  await expect(again.getByLabel('Group', { exact: true })).toHaveValue('session');
  await expect(again.getByRole('radio', { name: 'Newest first' })).toHaveAttribute('aria-checked', 'true');
  await expect(again.getByRole('button', { name: 'View (newest first)' })).toBeVisible();
  // A link that names its own order still wins.
  await again.keyboard.press('Escape');
  await again.goto(`${BASE}/#/grid?db=${encodeURIComponent(dbId)}&project=1&order=oldest&grouping=filter`);
  await expect(again.locator('.image-card')).toHaveCount(3, { timeout: 30_000 });
  await again.getByRole('button', { name: /^View/ }).click();
  await expect(again.getByRole('radio', { name: 'Oldest first' })).toHaveAttribute('aria-checked', 'true');
  await expect(again.getByLabel('Group', { exact: true })).toHaveValue('filter');
  await again.context().close();

  // Someone else sees none of it.
  const other = await signIn(browser, 'bob');
  await openGrid(other);
  await expect(other.getByLabel('Group', { exact: true })).toHaveValue('filter');
  await expect(other.getByRole('radio', { name: 'Oldest first' })).toHaveAttribute('aria-checked', 'true');
  await other.context().close();
});
