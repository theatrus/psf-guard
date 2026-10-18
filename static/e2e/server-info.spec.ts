import { expect, test, type Locator } from '@playwright/test';

// Documentation captures wait for deployment to the main server. These checks
// exercise layout and keyboard/mouse scrolling without saving screenshots.
test.use({ screenshot: 'off', trace: 'off' });

async function waitForScrollToSettle(body: Locator) {
  let previous: number | undefined;
  await expect.poll(async () => {
    const current = await body.evaluate(el => el.scrollTop);
    const settled = current === previous;
    previous = current;
    return settled;
  }, { intervals: [100] }).toBe(true);
}

for (const viewport of [{ width: 1280, height: 900 }, { width: 390, height: 700 }, { width: 844, height: 390 }]) {
  test(`server information scrolls long database lists at ${viewport.width} × ${viewport.height}`, async ({ page }) => {
    await page.setViewportSize(viewport);
    const catalogs = Array.from({ length: 18 }, (_, index) => ({
      id: `catalog-${index}`, name: `Observatory ${index + 1}`,
      database_path: `/catalogs/${'long-database-folder/'.repeat(12)}observatory-${index + 1}.sqlite`,
      image_directories: [`/images/${'long-image-folder/'.repeat(12)}first`, `/images/observatory-${index + 1}/second`],
      remote_image_upload: { enabled: false, token_configured: false, sync_enabled: false },
    }));
    await page.route('**/api/info', route => route.fulfill({ json: { success: true, data: {
      version: 'test', cache_directory: `/cache/${'long-cache-folder/'.repeat(15)}`, allow_database_management: false,
    } } }));
    await page.route('**/api/databases', route => route.fulfill({ json: { success: true, data: catalogs } }));
    await page.route('**/api/db/*/projects*', route => route.fulfill({ json: { success: true, data: [] } }));
    await page.route('**/api/db/*/target-navigation', route => route.fulfill({ json: { success: true, data: [] } }));
    await page.route('**/api/db/*/stats/overall', route => route.fulfill({ json: { success: true, data: {
      total_projects: 0, active_projects: 0, total_targets: 0, active_targets: 0,
      total_images: 0, accepted_images: 0, rejected_images: 0, pending_images: 0,
      total_desired: 0, files_found: 0, files_missing: 0, unique_filters: [], date_range: {}, recent_activity: [],
    } } }));
    await page.goto('/');
    const menu = page.getByRole('button', { name: 'Menu', exact: true });
    await menu.click();
    await page.getByRole('menuitem', { name: 'Server information' }).click();
    const dialog = page.getByRole('dialog', { name: 'Server information' });
    const body = dialog.locator('.dialog-body');
    const close = dialog.getByRole('button', { name: 'Close dialog' });
    const done = dialog.getByRole('button', { name: 'Done', exact: true });
    await expect(dialog.getByRole('heading', { name: 'Databases (18)' })).toBeVisible();
    await expect(close).toBeInViewport();
    await expect(done).toBeInViewport();
    expect(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    expect(await body.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    expect(await body.evaluate(el => el.scrollHeight > el.clientHeight)).toBe(true);
    await dialog.getByRole('region', { name: 'Server details' }).focus();
    await page.keyboard.press('PageDown');
    await expect.poll(() => body.evaluate(el => el.scrollTop)).toBeGreaterThan(0);
    await waitForScrollToSettle(body);
    const keyboardScrollTop = await body.evaluate(el => el.scrollTop);
    const bounds = (await body.boundingBox())!;
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
    await page.mouse.wheel(0, viewport.height);
    await expect.poll(() => body.evaluate(el => el.scrollTop)).toBeGreaterThan(keyboardScrollTop);
    await waitForScrollToSettle(body);
    await page.keyboard.press('End');
    await expect(dialog.getByText('/images/observatory-18/second', { exact: true })).toBeInViewport();
    await expect(close).toBeInViewport();
    await expect(done).toBeInViewport();
    expect(await page.locator('.app-main').evaluate(el => el.scrollTop)).toBe(0);
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(menu).toBeFocused();
  });
}
