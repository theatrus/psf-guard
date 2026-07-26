import { expect, test } from '@playwright/test';
import { resetDatabases } from './helpers';

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
});

test('overview shows the empty-state when no databases are configured', async ({
  page,
}) => {
  await page.goto('/');
  // The settings modal auto-opens with the welcome banner when management
  // is allowed AND nothing is configured yet.
  await expect(
    page.getByRole('heading', { name: /Welcome to PSF Guard/i })
  ).toBeVisible();
  // Close the modal to reach the underlying overview empty state.
  await page.getByRole('button', { name: 'Done' }).click();
  await expect(
    page.getByRole('heading', { name: 'No databases configured' })
  ).toBeVisible();
  // Both starting points are offered here, not just "open an existing
  // N.I.N.A. database".
  await expect(
    page.getByRole('button', { name: /New Database from Images/i })
  ).toBeVisible();
  // The empty state offers an action that re-opens settings — confirm it
  // actually triggers the modal again.
  await page.getByRole('button', { name: /Add Existing Database/i }).click();
  await expect(
    page.getByRole('heading', { name: /Welcome to PSF Guard/i })
  ).toBeVisible();
});

test('first run offers importing images as well as opening a N.I.N.A. database', async ({
  page,
}) => {
  await page.goto('/');
  await expect(
    page.getByRole('heading', { name: /Welcome to PSF Guard/i })
  ).toBeVisible();

  // Both choices sit in the welcome banner, above the fold. This is the whole
  // point: the import path used to live below a catalog-install panel and two
  // "add a catalog first" dead ends, so a first-time user never saw it.
  // Scope to the modal: the overview's own empty state sits behind it and
  // offers the same two choices.
  const modal = page.locator('.tauri-settings');
  const create = modal.getByRole('button', { name: /New Database from Images/i });
  const add = modal.getByRole('button', { name: /Add Existing Database/i });
  await expect(create).toBeVisible();
  await expect(add).toBeVisible();

  // Nothing that needs a database to be useful should be in the way.
  await expect(
    modal.getByText('Add a catalog before syncing with a remote PSF Guard.')
  ).toBeHidden();
  await expect(
    modal.getByText('Add a second catalog to merge data or send planning and grades.')
  ).toBeHidden();

  // The import choice opens the create form, which asks for image folders
  // rather than an existing database file.
  await create.click();
  await expect(
    modal.getByRole('heading', { name: 'New Database from Images' })
  ).toBeVisible();
  await expect(modal.getByText('N.I.N.A. Database File:')).toBeHidden();
  await expect(modal.getByText('Image Directories:')).toBeVisible();
});

test('the overview import action opens settings on the create form', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('heading', { name: /Welcome to PSF Guard/i }).waitFor();
  await page.getByRole('button', { name: 'Done' }).click();
  await page.getByRole('heading', { name: 'No databases configured' }).waitFor();

  // The intent rides along with the open-settings event, so the user lands on
  // the form they asked for instead of hunting for it again.
  await page.getByRole('button', { name: /New Database from Images/i }).click();
  await expect(
    page.getByRole('heading', { name: 'New Database from Images' })
  ).toBeVisible();
});

test('header Settings button is present in browser mode', async ({ page }) => {
  await page.goto('/');
  // Wait for the auto-popup to appear, then close it so we can find the
  // header button beneath. `exact: true` keeps this off any other control
  // whose name merely contains "Settings".
  await page.getByRole('heading', { name: /Welcome to PSF Guard/i }).waitFor();
  await page.getByRole('button', { name: 'Done' }).click();
  await expect(
    page.getByRole('button', { name: 'Settings', exact: true })
  ).toBeVisible();
});

test('header shows the PSF Guard logo', async ({ page }) => {
  await page.goto('/');

  const logo = page
    .getByRole('button', { name: 'PSF Guard' })
    .locator('.brand-logo');
  await expect(logo).toBeVisible();
  await expect(logo).toHaveAttribute('src', '/psf-guard.svg');

  // The logo declares its own size, and that size is square.
  //
  // Polling for a non-zero width first is not decoration: an <img> reports
  // zero until it has decoded, so reading straight after the visibility check
  // races the decode and fails intermittently.
  //
  // Squareness rather than a fixed number, because pinning the artwork's
  // exact intrinsic width is what broke this test when the logo was redrawn.
  // It still catches the thing that actually goes wrong — an SVG carrying
  // only a viewBox declares no intrinsic size, and the browser reports either
  // zero or its own 2:1 default in its place.
  await expect
    .poll(() => logo.evaluate((image: HTMLImageElement) => image.naturalWidth))
    .toBeGreaterThan(0);
  const intrinsic = await logo.evaluate((image: HTMLImageElement) => ({
    width: image.naturalWidth,
    height: image.naturalHeight,
  }));
  expect(intrinsic.width, 'the logo should declare a square intrinsic size').toBe(
    intrinsic.height
  );

  // And it is laid out at the size the header gives it.
  const box = await logo.boundingBox();
  expect(box?.width).toBeCloseTo(32, 0);
  expect(box?.height).toBeCloseTo(32, 0);
});

test('GET /api/info advertises database management is enabled', async ({
  request,
}) => {
  const res = await request.get('/api/info');
  expect(res.ok()).toBeTruthy();
  const body = await res.json();
  expect(body.data.allow_database_management).toBe(true);
});
