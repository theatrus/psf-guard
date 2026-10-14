import { expect, test, type Browser, type Page } from '@playwright/test';
import Database from 'better-sqlite3';
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
/** Project 1's frames: the fixture's three and enough more to scroll. */
const FRAMES = 3 + 45;

test.describe.configure({ mode: 'serial' });

test.beforeAll(async () => {
  const directory = path.join(tmpBase(), `user-preferences-${process.pid}`);
  fs.rmSync(directory, { recursive: true, force: true });
  fs.mkdirSync(directory, { recursive: true });
  const catalog = path.join(directory, 'scheduler.sqlite');
  fs.copyFileSync(fixtureDbPath(), catalog);
  // More frames like the fixture's own in project 1, an hour apart, after
  // them, so its one group runs well past the screen.
  const database = new Database(catalog);
  try {
    const last = database.prepare(
      'SELECT targetId, acquireddate, filtername, metadata FROM acquiredimage WHERE projectId=1 ORDER BY acquireddate DESC LIMIT 1',
    ).get() as { targetId: number; acquireddate: number; filtername: string; metadata: string };
    const insert = database.prepare(
      'INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata) VALUES(?,1,?,?,?,0,?)',
    );
    for (let index = 1; index <= FRAMES - 3; index++) {
      insert.run(5000 + index, last.targetId, last.acquireddate + index * 3600, last.filtername, last.metadata);
    }
  } finally {
    database.close();
  }
  const registry = path.join(directory, 'registry.json');
  const passwordFile = path.join(directory, 'password');
  fs.writeFileSync(passwordFile, `${PASSWORD}\n`);
  // A viewer, to show a read-only account saves its own view, and editors.
  for (const [name, role] of [['alice', 'read-only'], ['bob', 'read-write'], ['carol', 'read-write']]) {
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

/** A browser with nothing of its own yet, signed in as `name`. `before`
 *  runs on the new page before it first loads. */
async function signIn(browser: Browser, name: string, before?: (page: Page) => Promise<void>): Promise<Page> {
  const context = await browser.newContext({ storageState: { cookies: [], origins: [] } });
  const page = await context.newPage();
  await before?.(page);
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

async function openGrid(page: Page, { view = true }: { view?: boolean } = {}) {
  // The link names no order and no grouping: the saved ones apply.
  await page.goto(`${BASE}/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card')).toHaveCount(FRAMES, { timeout: 30_000 });
  if (view) await page.getByRole('button', { name: /^View/ }).click();
}

/** The preferences this page has saved so far, merged as the server
 *  merges PATCHes; a PUT would fail the spec, since this build never sends
 *  one. */
function savedFrom(page: Page): () => Record<string, unknown> {
  const saved: Record<string, unknown> = {};
  page.on('requestfinished', (request) => {
    if (!request.url().endsWith('/api/me/preferences')) return;
    expect(request.method()).not.toBe('PUT');
    if (request.method() !== 'PATCH') return;
    Object.assign(saved, (JSON.parse(request.postData() ?? '{}') as { preferences: object }).preferences);
  });
  return () => saved;
}

test('a user\'s order and grouping come back in a fresh browser, and stay theirs', async ({ browser }) => {
  const first = await signIn(browser, 'alice');
  const saved = savedFrom(first);
  await openGrid(first);
  await expect(first.getByLabel('Group', { exact: true })).toHaveValue('filter');
  await first.getByRole('radio', { name: 'Newest first' }).click();
  await first.getByLabel('Group', { exact: true }).selectOption('session');
  // Each change goes as just its own keys.
  await expect.poll(saved).toEqual({ imageOrder: 'newest', imageGrouping: 'session' });
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
  await expect(again.locator('.image-card')).toHaveCount(FRAMES, { timeout: 30_000 });
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

test('a saved order that arrives after the frames drew puts the cursor on the first card, with the page still at the top', async ({ browser }) => {
  // Carol chose newest first on one browser.
  const first = await signIn(browser, 'carol');
  const saved = await first.evaluate(async () => (await fetch('/api/me/preferences', {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ schema_version: 1, preferences: { imageOrder: 'newest' } }),
  })).status);
  expect(saved).toBe(200);
  await first.context().close();

  // On a fresh one, her document is slow to come, from sign-in on: the
  // frames draw first, oldest first, with the cursor on the first card.
  let answer: () => void = () => {};
  const answered = new Promise<void>((resolve) => { answer = resolve; });
  const page = await signIn(browser, 'carol', (fresh) => fresh.route('**/api/me/preferences', async (route) => {
    if (route.request().method() === 'GET') await answered;
    // A read the page dropped as it reloaded has nothing to continue.
    await route.continue().catch(() => {});
  }));
  await openGrid(page, { view: false });
  const ids = () => page.locator('.image-card-wrapper').evaluateAll(
    (cards) => cards.map((card) => card.getAttribute('data-image-id')),
  );
  const oldest = await ids();
  await expect(page).toHaveURL(new RegExp(`current=${oldest[0]}(&|$)`));
  const main = page.locator('.app-main');
  expect(await main.evaluate((element) => element.scrollTop)).toBe(0);

  // Then it comes: newest first, and the view, untouched, flips at rest.
  answer();
  await expect.poll(ids).toEqual([...oldest].reverse());
  await expect(page.locator('.image-card-wrapper').first()).toHaveClass(/current-selection/);
  await expect(page).toHaveURL(new RegExp(`current=${oldest[oldest.length - 1]}(&|$)`));
  await expect(page.locator('.image-card-wrapper').first()).toBeInViewport();
  // Give a late scroll the chance to show itself.
  await page.waitForTimeout(500);
  expect(await main.evaluate((element) => element.scrollTop)).toBe(0);
  await expect(page.getByRole('button', { name: 'View (newest first)' })).toBeVisible();
  // The link still names no order: the saved one decides.
  expect(page.url()).not.toMatch(/order=/);
  await page.context().close();
});
