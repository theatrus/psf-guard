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
  // The empty state offers an action that re-opens settings — confirm it
  // actually triggers the modal again.
  await page.getByRole('button', { name: /Open Settings/i }).click();
  await expect(
    page.getByRole('heading', { name: /Welcome to PSF Guard/i })
  ).toBeVisible();
});

test('header Settings button is present in browser mode', async ({ page }) => {
  await page.goto('/');
  // Wait for the auto-popup to appear, then close it so we can find the
  // header button beneath. Use `exact: true` because the overview's empty
  // state also has an "Open Settings" button.
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
  await expect(logo).toHaveAttribute('src', /^data:image\/png;base64,/);
  await expect
    .poll(() => logo.evaluate((image: HTMLImageElement) => image.naturalWidth))
    .toBe(32);
});

test('GET /api/info advertises database management is enabled', async ({
  request,
}) => {
  const res = await request.get('/api/info');
  expect(res.ok()).toBeTruthy();
  const body = await res.json();
  expect(body.data.allow_database_management).toBe(true);
});
