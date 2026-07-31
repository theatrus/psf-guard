import { expect, test } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';

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
        can_compute: false,
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
        can_compute: false,
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

test('editor manages browser users from a separate Settings tab', async ({ page }) => {
  let users = [
    { username: 'editor', role: 'read_write', managed: true },
    { username: 'recovery', role: 'read_write', managed: false },
  ];
  await page.route('**/api/auth/status', (route) =>
    route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify(apiResponse({
        authentication_required: true,
        authenticated: true,
        role: 'read_write',
        username: 'editor',
        can_compute: true,
      })),
    })
  );
  await page.route('**/api/auth/users', async (route) => {
    if (route.request().method() === 'POST') {
      const request = route.request().postDataJSON();
      users = [...users, {
        username: request.username,
        role: request.role,
        managed: true,
      }];
    }
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify(apiResponse(users)),
    });
  });

  await page.goto('/');
  const usersTab = page.getByRole('tab', { name: 'Users' });
  await usersTab.click();

  await expect(page.getByRole('heading', { name: 'Browser users' })).toBeVisible();
  await expect(page.getByText('TOML bootstrap', { exact: true })).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Remove' }).first()
  ).toBeDisabled();
  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    const docs = path.resolve(process.cwd(), '..', 'docs');
    fs.mkdirSync(docs, { recursive: true });
    await page.locator('.tauri-settings .modal-content').screenshot({
      path: path.join(docs, 'server-users.png'),
    });
  }

  await page.getByRole('button', { name: '+ Add user' }).click();
  await page.getByLabel('Username').fill('viewer');
  await page.getByLabel('Password', { exact: true }).fill('long-viewer-password');
  await page.getByLabel('Confirm password').fill('long-viewer-password');
  await page.getByRole('button', { name: 'Save user' }).click();

  await expect(page.getByText('viewer', { exact: true })).toBeVisible();
});
