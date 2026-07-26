import { expect, test } from '@playwright/test';
import { fixtureDbPath, fixtureImageDir, resetDatabases } from './helpers';

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
});

test('add a database via settings → see it in the Overview', async ({ page }) => {
  await page.goto('/');

  // Wait for settings to auto-open (gated on a couple of async API calls).
  await page.getByRole('heading', { name: /Welcome to PSF Guard/i }).waitFor();

  // First run offers two starting points and picks neither for you.
  await page
    .locator('.tauri-settings')
    .getByRole('button', { name: /Add Existing Database/i })
    .click();

  // The form is now visible. Fill the inputs.
  await page
    .getByPlaceholder('e.g. Imaging Rig (defaults to filename)')
    .fill('e2e Rig');
  await page
    .getByPlaceholder('Select or enter database path')
    .fill(fixtureDbPath());

  // Add the image directory via the manual-add fallback in browser mode.
  await page
    .getByPlaceholder('Type an absolute path and press Add')
    .fill(fixtureImageDir());
  await page.getByRole('button', { name: 'Add', exact: true }).click();

  // The directory should appear in the list.
  await expect(page.getByText(fixtureImageDir())).toBeVisible();

  // Save the new entry.
  await page.getByRole('button', { name: 'Add Database', exact: true }).click();

  // Wait for the saved status (proves the round-trip succeeded), and the row
  // should appear in the "Configured Databases" list inside the modal.
  await expect(page.getByText('Saved.')).toBeVisible();
  await expect(page.getByText('e2e Rig').first()).toBeVisible();

  // Close the modal and verify the Overview reflects both projects from the
  // new database. The merged-overview hooks fan out per-DB queries; give them
  // a moment to settle after the registry mutation.
  await page.getByRole('button', { name: 'Done' }).click();
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });
  await expect(
    page.getByRole('button', { name: 'Open Project Alpha image grid' })
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Open Project Beta image grid' })
  ).toBeVisible();
  await expect(page.locator('.project-database', { hasText: 'e2e Rig' })).toHaveCount(2);
});

test('add then remove a database', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('heading', { name: /Welcome to PSF Guard/i }).waitFor();
  await page
    .locator('.tauri-settings')
    .getByRole('button', { name: /Add Existing Database/i })
    .click();

  await page.getByPlaceholder('Select or enter database path').fill(fixtureDbPath());
  // The backend rejects a DB without image dirs, so populate one.
  await page
    .getByPlaceholder('Type an absolute path and press Add')
    .fill(fixtureImageDir());
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await page.getByRole('button', { name: 'Add Database', exact: true }).click();

  // Saved status appears once the POST completes; the row follows.
  await expect(page.getByText('Saved.')).toBeVisible();
  const row = page.locator('.db-row').first();
  await expect(row).toBeVisible();

  page.on('dialog', (dialog) => dialog.accept());
  await row.getByRole('button', { name: 'Remove' }).click();

  // After removing the only DB, the registry is empty again and the welcome
  // banner returns with both starting points on offer.
  await expect(page.getByText(/Removed/i)).toBeVisible();
  await expect(
    page.getByRole('heading', { name: /Welcome to PSF Guard/i })
  ).toBeVisible();
  await expect(
    page
      .locator('.tauri-settings')
      .getByRole('button', { name: /New Database from Images/i })
  ).toBeVisible();
});
