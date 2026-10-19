import { expect, test, type Page, type TestInfo } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { registerFixtureDb, resetDatabases, waitForCacheReady, openSettings } from './helpers';

// A common phone: iPhone 14 / Pixel-class logical viewport.
const PHONE = { width: 390, height: 844 };
test.use({ viewport: PHONE, isMobile: true, hasTouch: true });

let dbId: string;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'Imaging Rig e2e',
    slug: 'imaging-rig-e2e',
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

/**
 * The one mobile invariant every view must hold: the page body never scrolls
 * sideways. Wide content (tables, strips, canvases) must scroll inside its
 * own container instead. On failure, name the widest offenders so the fix
 * starts from the right selector.
 *
 * Also writes a full-page screenshot per view — into the test output dir,
 * and into PSF_GUARD_E2E_SHOT_DIR when set — so a human can review the
 * layout, which no width assertion can do alone.
 */
async function expectPhoneFit(page: Page, testInfo: TestInfo, name: string) {
  // Let late-arriving previews and fonts settle before measuring.
  await page.waitForLoadState('networkidle').catch(() => {});

  const shot = await page.screenshot({ fullPage: true });
  await testInfo.attach(`${name}.png`, { body: shot, contentType: 'image/png' });
  const shotDir = process.env.PSF_GUARD_E2E_SHOT_DIR;
  if (shotDir) {
    fs.mkdirSync(shotDir, { recursive: true });
    fs.writeFileSync(path.join(shotDir, `${name}.png`), shot);
  }

  const layout = await page.evaluate(() => {
    const limit = window.innerWidth + 1;
    const offenders: string[] = [];
    for (const el of Array.from(document.querySelectorAll<HTMLElement>('*'))) {
      const rect = el.getBoundingClientRect();
      if ((rect.right > limit || rect.left < -1) && rect.width > 24) {
        const cls = typeof el.className === 'string' ? el.className : '';
        offenders.push(
          `<${el.tagName.toLowerCase()} class="${cls.split(' ').slice(0, 2).join(' ')}"> ` +
            `left=${Math.round(rect.left)} right=${Math.round(rect.right)}`
        );
        if (offenders.length >= 10) break;
      }
    }
    return {
      scrollWidth: document.documentElement.scrollWidth,
      innerWidth: window.innerWidth,
      offenders,
    };
  });

  expect(
    layout.scrollWidth,
    `${name} scrolls sideways (${layout.scrollWidth}px page in a ` +
      `${layout.innerWidth}px viewport). Widest elements:\n` +
      layout.offenders.join('\n')
  ).toBeLessThanOrEqual(layout.innerWidth + 1);
}

test('overview fits a phone', async ({ page }, testInfo) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });
  await expectPhoneFit(page, testInfo, 'overview');
  // Scroll the main content column, not the header the pointer starts over.
  await page.mouse.move(195, 500);
  await page.mouse.wheel(0, 8000);
  await expectPhoneFit(page, testInfo, 'overview-bottom');
});

test('compact overview fits a phone', async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    window.localStorage.setItem(
      'psf-guard.display-preferences',
      JSON.stringify({ libraryDensity: 'compact' })
    );
  });
  await page.goto('/');
  const rows = page.getByTestId('library-row');
  await expect(rows).toHaveCount(2, { timeout: 15_000 });
  await rows.first().scrollIntoViewIfNeeded();
  await expectPhoneFit(page, testInfo, 'overview-compact');
  // A stacked row: the name line first, then its pills below it.
  const name = await rows.first().locator(`[data-column='name']`).boundingBox();
  const progress = await rows.first().locator(`[data-column='progress']`).boundingBox();
  expect(name).not.toBeNull();
  expect(progress).not.toBeNull();
  expect(progress!.y).toBeGreaterThanOrEqual(name!.y + name!.height);
});

test('image grid fits a phone', async ({ page }, testInfo) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  const firstCard = page.locator('.image-card').first();
  await expect(firstCard).toBeVisible({ timeout: 15_000 });
  await expectPhoneFit(page, testInfo, 'grid');
  await firstCard.scrollIntoViewIfNeeded();
  await expectPhoneFit(page, testInfo, 'grid-cards');
});

test('grid statistics fit a phone', async ({ page }, testInfo) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card').first()).toBeVisible({
    timeout: 15_000,
  });
  await page.getByRole('button', { name: 'Stats' }).click();
  const stats = page.locator('.stats-dashboard');
  await expect(stats).toBeVisible();
  await stats.scrollIntoViewIfNeeded();
  await expectPhoneFit(page, testInfo, 'grid-stats');
});

test('export dialog fits a phone', async ({ page }, testInfo) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });
  const exportButton = page.getByRole('button', { name: /Export/ }).first();
  await exportButton.scrollIntoViewIfNeeded();
  await exportButton.click();
  await expect(page.locator('.export-dialog')).toBeVisible();
  await expectPhoneFit(page, testInfo, 'export-dialog');
});

test('project picker opens usable on a phone', async ({ page }, testInfo) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card').first()).toBeVisible({
    timeout: 15_000,
  });
  await page.getByRole('button', { name: /Project \/ target:/ }).click();
  await expect(
    page.getByPlaceholder('Type to find a project or target')
  ).toBeVisible();
  await expectPhoneFit(page, testInfo, 'project-picker');
});

test('image detail fits a phone', async ({ page }, testInfo) => {
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);
  // The grid's card for the same frame can be mounted under the detail view
  // with the same alt text, so name the detail image itself.
  await expect(
    page.locator('img.detail-main-image[alt="Alpha M44 - B"]')
  ).toBeVisible({
    timeout: 15_000,
  });
  await expectPhoneFit(page, testInfo, 'detail');
  await page
    .locator('.detail-info')
    .evaluate((el) => el.scrollTo(0, el.scrollHeight));
  await expectPhoneFit(page, testInfo, 'detail-info-end');
});

test('comparison fits a phone', async ({ page }, testInfo) => {
  await page.goto(
    `/#/compare/1/2?db=${encodeURIComponent(dbId)}&project=1`
  );
  await expect(page.locator('.comparison-container')).toBeVisible({
    timeout: 15_000,
  });
  await expectPhoneFit(page, testInfo, 'compare');
});

test('the grid View panel and its Sort fit a phone', async ({ page }, testInfo) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card').first()).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: 'View', exact: true }).click();
  const sort = page.getByRole('radiogroup', { name: 'Sort' });
  await expect(sort).toBeVisible();
  await sort.getByRole('radio', { name: 'Newest first' }).click();
  await expect(page).toHaveURL(/order=newest/);
  // Nobody had moved: the page stays put, so the panel stays in view and
  // the cursor sits on the new first card.
  await expect(sort).toBeInViewport();
  await expect(page.locator('.image-card-wrapper').first()).toHaveClass(/current-selection/);
  // The View button, an icon on a phone, still says the order is on.
  await expect(page.getByRole('button', { name: 'View (newest first)' })).toHaveClass(/is-set/);
  await expectPhoneFit(page, testInfo, 'grid-view-sort');
});

test('sequence view fits a phone', async ({ page }, testInfo) => {
  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.sequence-view')).toBeVisible({ timeout: 15_000 });
  await expectPhoneFit(page, testInfo, 'sequence');
});

test('settings modal fits a phone', async ({ page }, testInfo) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });
  await openSettings(page);
  await expect(page.locator('.tauri-settings').first()).toBeVisible();
  await expectPhoneFit(page, testInfo, 'settings');
});

test('help overlay fits a phone', async ({ page }, testInfo) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });
  // A phone has no keyboard; the header menu is the mobile path.
  await page.getByRole('button', { name: 'Menu', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Keyboard shortcuts' }).click();
  await expect(page.getByText('Keyboard Shortcuts').first()).toBeVisible();
  await expectPhoneFit(page, testInfo, 'help');
});

test('the phone header takes two rows and tucks away while the page scrolls down', async ({ page }) => {
  // Short enough that the three fixture frames scroll well past the header.
  await page.setViewportSize({ width: PHONE.width, height: 520 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card').first()).toBeVisible({ timeout: 15_000 });
  const header = page.locator('.app-header');
  const height = (await header.boundingBox())!.height;
  expect(height, 'the header holds two rows').toBeLessThan(130);
  // The logo opens the Library, so the header leaves its tab out; the view
  // in use is named by its highlighted icon.
  await expect(header.getByRole('button', { name: 'Library', exact: true })).toBeHidden();
  const images = header.getByRole('button', { name: 'Images', exact: true });
  await expect(images).toHaveAttribute('aria-current', 'page');
  // Icon only, the current view's too, and a tap opens no name.
  await expect(images.locator('.view-tab-name')).toBeHidden();
  await images.tap();
  await expect(images.locator('.view-tab-name')).toBeHidden();
  expect((await images.boundingBox())!.width).toBeLessThanOrEqual(40);

  const main = page.locator('.app-main');
  await main.evaluate((el) => el.scrollBy(0, 300));
  await expect(header).toHaveClass(/is-tucked/);
  await expect.poll(async () => (await header.boundingBox())!.y + (await header.boundingBox())!.height)
    .toBeLessThanOrEqual(1);
  await main.evaluate((el) => el.scrollBy(0, -60));
  await expect(header).not.toHaveClass(/is-tucked/);
  await expect.poll(async () => (await header.boundingBox())!.y).toBeGreaterThanOrEqual(0);

  await header.getByRole('button', { name: 'PSF Guard' }).click();
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({ timeout: 15_000 });
});

test('a phone grades and steps from the bar under the detail panels', async ({ page, request }) => {
  // Advance on grade is on by default and moves to the next image 100 ms
  // after each grade, racing the Next and Previous clicks below.
  await page.addInitScript(() => {
    window.localStorage.setItem(
      'psf-guard.display-preferences',
      JSON.stringify({ advanceOnGrade: false })
    );
  });
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('img.detail-main-image[alt="Alpha M44 - B"]')).toBeVisible({ timeout: 15_000 });
  const bar = page.getByRole('group', { name: 'Grade and step' });
  await expect(bar).toBeInViewport();
  // The panel's own buttons give way to the bar.
  await expect(page.locator('.detail-actions')).toBeHidden();
  const status = async () => {
    const response = await request.get(`/api/db/${encodeURIComponent(dbId)}/images/1`);
    return (await response.json()).data.grading_status;
  };
  await bar.getByRole('button', { name: 'Reject', exact: true }).click();
  await expect.poll(status).toBe(2);
  await bar.getByRole('button', { name: 'Unmark', exact: true }).click();
  await expect.poll(status).toBe(0);
  await bar.getByRole('button', { name: 'Next image' }).click();
  await expect(page).toHaveURL(/#\/detail\/2\?/);
  await bar.getByRole('button', { name: 'Previous image' }).click();
  await expect(page).toHaveURL(/#\/detail\/1\?/);
});

test('a phone grid toolbar keeps one row, with Undo under its ⋯ menu', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card').first()).toBeVisible({ timeout: 15_000 });
  const toolbar = page.getByRole('region', { name: 'Image tools' });
  await expect(toolbar.locator('.undo-redo-toolbar')).toBeHidden();
  await expect(toolbar.getByRole('button', { name: /Compare/ })).toBeHidden();
  await expect(toolbar.getByText('0 selected')).toBeHidden();
  const tools = toolbar.locator('.view-toolbar-bar > *:visible');
  const tops = await tools.evaluateAll((items) => items.map((item) => Math.round(item.getBoundingClientRect().top)));
  expect(new Set(tops).size, 'the control row stays one row').toBe(1);
  await toolbar.getByRole('button', { name: 'More tools' }).click();
  await expect(toolbar.getByRole('button', { name: 'Undo', exact: true })).toBeVisible();
  await expect(toolbar.getByRole('button', { name: 'Redo', exact: true })).toBeVisible();
});
