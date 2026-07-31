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
  await page.setViewportSize({ width: 320, height: 700 });

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

  const cardBox = await page.locator('.auth-card').boundingBox();
  const usernameBox = await page.getByLabel('Username').boundingBox();
  const passwordBox = await page.getByLabel('Password').boundingBox();
  expect(cardBox).not.toBeNull();
  for (const field of [usernameBox, passwordBox]) {
    expect(field).not.toBeNull();
    expect(field!.x).toBeGreaterThanOrEqual(cardBox!.x);
    expect(field!.x + field!.width).toBeLessThanOrEqual(cardBox!.x + cardBox!.width);
  }

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
    { username: 'editor', role: 'read_write' },
    { username: 'reviewer', role: 'read_only', email: 'reviewer@example.com' },
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
        email: request.email,
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
  await expect(page.getByText('reviewer', { exact: true })).toBeVisible();
  await expect(page.getByText('reviewer@example.com', { exact: true })).toBeVisible();
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

  const reviewerRow = page.locator('.user-row').filter({ hasText: 'reviewer' });
  await reviewerRow.getByRole('button', { name: 'Edit' }).click();
  await expect(page.getByLabel('Email (optional)')).toHaveValue('reviewer@example.com');
  await page.getByRole('button', { name: 'Cancel' }).click();

  await page.getByRole('button', { name: '+ Add user' }).click();
  await page.getByLabel('Username').fill('viewer');
  await page.getByLabel('Email (optional)').fill('viewer@example.com');
  await page.getByLabel('Password', { exact: true }).fill('long-viewer-password');
  await page.getByLabel('Confirm password').fill('long-viewer-password');
  await page.getByRole('button', { name: 'Save user' }).click();

  await expect(page.getByText('viewer', { exact: true })).toBeVisible();
  await expect(page.getByText('viewer@example.com', { exact: true })).toBeVisible();
});
