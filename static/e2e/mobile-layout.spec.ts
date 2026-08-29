import { expect, test, type Page, type TestInfo } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

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
  await expect(page.getByRole('img', { name: 'Alpha M44 - B' })).toBeVisible({
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
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.locator('.tauri-settings').first()).toBeVisible();
  await expectPhoneFit(page, testInfo, 'settings');
});

test('help overlay fits a phone', async ({ page }, testInfo) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });
  // A phone has no keyboard; the Help button is the mobile path.
  await page.getByRole('button', { name: 'Help' }).click();
  await expect(page.getByText('Keyboard Shortcuts').first()).toBeVisible();
  await expectPhoneFit(page, testInfo, 'help');
});
