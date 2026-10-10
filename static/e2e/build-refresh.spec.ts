import { expect, test, type Page } from '@playwright/test';
import { registerFixtureDb, resetDatabases } from './helpers';

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  await registerFixtureDb(request, { name: 'Imaging Rig e2e', slug: 'imaging-rig-e2e' });
});

test('the page is checked on every load and names its build', async ({ request }) => {
  const page = await request.get('/');
  expect(page.headers()['cache-control']).toBe('no-cache');
  const build = (await page.text()).match(/<meta name="psf-guard-build" content="([^"]+)"/)?.[1];
  expect(build).toBeTruthy();
  expect(page.headers()['x-psf-guard-build']).toBe(build);
  expect((await request.get('/api/info')).headers()['x-psf-guard-build']).toBe(build);

  const script = (await page.text()).match(/src="\/(assets\/[^"]+\.js)"/)?.[1];
  expect((await request.get(`/${script}`)).headers()['cache-control']).toContain('immutable');
  // A script only an older page asks for is gone: a 404, not a page of HTML.
  expect((await request.get('/assets/index-0ldBu1ld.js')).status()).toBe(404);
});

const mark = (page: Page) => page.evaluate(() => { (window as unknown as { stale?: boolean }).stale = true; });
const marked = (page: Page) => page.evaluate(() => (window as unknown as { stale?: boolean }).stale === true);

test('an open tab loads a newer server build when it changes views', async ({ page }) => {
  // The server was updated under the open tab: its replies name another build.
  await page.route('**/api/**', async route => {
    const response = await route.fetch();
    await route.fulfill({ response, headers: { ...response.headers(), 'x-psf-guard-build': 'newer' } });
  });
  await page.goto('/#/');
  const notice = page.getByRole('status').filter({ hasText: 'PSF Guard was updated' });
  await expect(notice).toBeVisible();

  await mark(page);
  await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Sky' }).click();
  await page.waitForFunction(() => !(window as unknown as { stale?: boolean }).stale);
  await expect(page).toHaveURL(/#\/sky/);

  // The reload brought this page back (here, the route above still names
  // another build), so it does not reload again on every change of view.
  await expect(notice).toBeVisible();
  await mark(page);
  await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Library' }).click();
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({ timeout: 15_000 });
  expect(await marked(page)).toBe(true);
});
