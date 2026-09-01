import { expect, test } from '@playwright/test';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

// The Scoring control's absolute reject limits, exercised against the real
// server. The fixture's Alpha sequence has HFRs 2.4 / 2.5 / 2.6 and star
// counts 520 / 510 / 500, so a ceiling of 2.55 or a floor of 505 trips
// exactly the third frame.

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

test('an HFR reject limit flags the soft frame, survives reload, and resets', async ({
  page,
}) => {
  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);
  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await expect(page.locator('.sequence-image-card.below-threshold')).toHaveCount(0);

  const scoring = page.locator('.scoring-penalty-control');
  await expect(scoring.locator('summary')).toHaveText('Scoring');
  await scoring.locator('summary').click();

  const hfrInput = page.getByLabel('HFR above');
  await hfrInput.pressSequentially('2.55');
  await expect(hfrInput).toHaveAttribute('step', 'any');
  expect(
    await hfrInput.evaluate((input: HTMLInputElement) => input.validity.stepMismatch)
  ).toBe(false);
  await hfrInput.press('Enter');

  // The HFR 2.6 frame drops to the capped score and carries the limit reason.
  const capped = page.locator('.sequence-image-card.below-threshold');
  await expect(capped).toHaveCount(1, { timeout: 15_000 });
  await expect(scoring.locator('summary')).toHaveText('Scoring *');
  await capped.getByRole('button', { name: 'Show quality reason' }).click();
  const popover = page.getByRole('dialog', { name: 'Quality reason' });
  await expect(popover).toContainText('[Auto] HFR limit');
  await expect(popover).toContainText('above limit 2.55');
  await popover.getByRole('button', { name: 'Close quality reason' }).click();

  // The preference is remembered: a reload scores the same way.
  await page.reload();
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await expect(page.locator('.sequence-image-card.below-threshold')).toHaveCount(1);
  await page.locator('.scoring-penalty-control summary').click();
  await expect(page.getByLabel('HFR above')).toHaveValue('2.55');

  // Reset restores the calibrated behavior.
  await page.getByRole('button', { name: 'Reset to defaults' }).click();
  await expect(page.locator('.sequence-image-card.below-threshold')).toHaveCount(0, {
    timeout: 15_000,
  });
  await expect(page.locator('.scoring-penalty-control summary')).toHaveText('Scoring');
  await expect(page.getByLabel('HFR above')).toHaveValue('');
});

test('a star-count floor flags the sparse frame', async ({ page }) => {
  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);
  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });

  await page.locator('.scoring-penalty-control summary').click();
  const starsInput = page.getByLabel('Stars below');
  await starsInput.pressSequentially('505');
  await starsInput.press('Enter');

  const capped = page.locator('.sequence-image-card.below-threshold');
  await expect(capped).toHaveCount(1, { timeout: 15_000 });
  await capped.getByRole('button', { name: 'Show quality reason' }).click();
  const popover = page.getByRole('dialog', { name: 'Quality reason' });
  await expect(popover).toContainText('[Auto] Star count limit');
  await expect(popover).toContainText('500 star(s) below limit 505');
});

test('a sub-1 HFR limit can be typed digit by digit, leading zero first', async ({
  page,
}) => {
  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);
  await expect(page.locator('.sequence-image-card')).toHaveCount(3, {
    timeout: 15_000,
  });

  await page.locator('.scoring-penalty-control summary').click();
  const hfrInput = page.getByLabel('HFR above');
  // Regression: the first keystroke of "0.8" used to be coerced to "off"
  // and wiped, making sub-1 limits impossible to type.
  await hfrInput.pressSequentially('0.8');
  await expect(hfrInput).toHaveValue('0.8');
  await hfrInput.press('Enter');

  // Every fixture frame has HFR above 0.8, so all three are capped.
  await expect(page.locator('.sequence-image-card.below-threshold')).toHaveCount(3, {
    timeout: 15_000,
  });
});
