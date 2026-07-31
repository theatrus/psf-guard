import { expect, test } from '@playwright/test';

const apiResponse = (data: unknown) => ({
  success: true,
  data,
  error: null,
  status: 'ready',
});

test('viewer signs in through the app and gets a read-only session', async ({ page }) => {
  let authenticated = false;

  await page.route('**/api/auth/status', (route) =>
    route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify(apiResponse({
        authentication_required: true,
        authenticated,
        ...(authenticated ? { role: 'read_only', username: 'viewer' } : {}),
      })),
    })
  );
  await page.route('**/api/auth/login', async (route) => {
    const credentials = route.request().postDataJSON();
    expect(credentials).toEqual({ username: 'viewer', password: 'secret' });
    authenticated = true;
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify(apiResponse({
        authentication_required: true,
        authenticated: true,
        role: 'read_only',
        username: 'viewer',
      })),
    });
  });
  await page.route('**/api/auth/logout', async (route) => {
    authenticated = false;
    await route.fulfill({ status: 204 });
  });

  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Sign in to PSF Guard' })).toBeVisible();

  await page.getByLabel('Username').fill('viewer');
  await page.getByLabel('Password').fill('secret');
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page.getByText('Read only', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Settings', exact: true })).toHaveCount(0);

  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to PSF Guard' })).toBeVisible();
});
