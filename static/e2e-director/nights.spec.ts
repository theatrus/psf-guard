import { expect, test } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { addRig, rigDatabase, shot } from './nightsFixtures';

test('Nights shows every rig across the coming nights, a night\'s schedule, and marks', async ({ page, request }, testInfo) => {
  test.setTimeout(120_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const slugs = [`nights-north-${randomUUID().slice(0, 6)}`, `nights-south-${randomUUID().slice(0, 6)}`];
  try {
    await addRig(request, 'North rig', slugs[0], rigDatabase([
      { name: 'Heart Nebula', priority: 2, target: 'IC 1805', ra_hours: 2.5467, dec: 61.45, filters: [['Ha', 120], ['OIII', 120]], switchEvery: 3 },
      { name: 'Andromeda', priority: 1, target: 'M 31', ra_hours: 0.7123, dec: 41.27, filters: [['L', 200]], moon: true },
      { name: 'Soul Nebula', priority: 0, target: 'IC 1848', ra_hours: 2.8533, dec: 60.4, filters: [['Ha', 80]] },
      // Never above 25° from 34° north, so no night applies to it.
      { name: 'Carina Nebula', priority: 0, target: 'NGC 3372', ra_hours: 10.7514, dec: -59.87, filters: [['Ha', 120]] },
    ]), { latitude_degrees: 34.2, longitude_degrees: -118.3 });
    await addRig(request, 'South rig', slugs[1], rigDatabase([
      { name: 'Orion', priority: 1, target: 'M 42', ra_hours: 5.588, dec: -5.39, filters: [['Ha', 150], ['SII', 150]], switchEvery: 2 },
    ]), { latitude_degrees: -31.3, longitude_degrees: 149.1 });

    await page.setViewportSize({ width: 1440, height: 1100 });
    // Ten days on, so the nights are always ahead and fully planned.
    const ahead = new Date(Date.now() + 10 * 86_400_000);
    const from = `${ahead.getFullYear()}-${String(ahead.getMonth() + 1).padStart(2, '0')}-${String(ahead.getDate()).padStart(2, '0')}`;
    await page.goto(`/#/rigs?rigsTab=nights&nightsFrom=${from}`);
    const grid = page.getByTestId('nights-grid');
    await expect(grid).toBeVisible({ timeout: 30_000 });
    await expect(grid.getByText('North rig', { exact: true })).toBeVisible();
    await expect(grid.getByText('South rig', { exact: true })).toBeVisible();
    await expect(grid.getByText('Target Scheduler').first()).toBeVisible();
    const north = grid.getByRole('row').filter({ hasText: 'North rig' });
    const cells = north.getByRole('button');
    await expect(cells).toHaveCount(7);
    await expect(cells.first()).toContainText('planned');
    await shot(page, 'nights-grid', name => testInfo.outputPath(name));

    // A night's schedule, with the planner's reasons for a block.
    await cells.nth(1).click();
    const detail = page.getByRole('region', { name: 'Night details' });
    await expect(detail.getByRole('heading', { name: 'Target Scheduler schedule' })).toBeVisible();
    const timeline = detail.getByRole('group', { name: 'Schedule' });
    const blocks = timeline.locator('.nights-block');
    await expect(blocks.first()).toBeVisible({ timeout: 30_000 });
    await blocks.first().click();
    const reasons = detail.getByRole('region', { name: 'Why this target' });
    await expect(reasons).toBeVisible();
    await expect(reasons.getByText('Chosen')).toBeVisible();
    await shot(page, 'nights-schedule', name => testInfo.outputPath(name));

    // The plan table lists what the night applies to and folds the rest.
    const plans = detail.getByRole('region', { name: /^This night: [1-9]\d* targets? planned/ });
    await expect(plans).toBeVisible();
    await expect(plans.getByRole('row').filter({ hasText: 'Heart Nebula' }).getByRole('cell', { name: /^\d+ frames?$/ })).toBeVisible();
    // Andromeda may fold too, on nights the Moon turns it down.
    await plans.getByText(/^Not this night: \d+ targets?$/).click();
    await expect(plans.getByRole('listitem').filter({ hasText: 'Carina Nebula · NGC 3372' })).toContainText('not above its horizon and altitude limits during darkness');
    await expect(plans.getByRole('row').filter({ hasText: 'Carina Nebula' })).toHaveCount(0);
    await shot(plans, 'nights-plans', name => testInfo.outputPath(name), false);

    // Mark the night clear only from 22:00 to 02:00 for this rig.
    await detail.getByLabel('Limited').check();
    await detail.getByLabel('From').fill('22:00');
    await detail.getByLabel('To').fill('02:00');
    await detail.getByLabel('Note').fill('high cirrus');
    await detail.getByRole('button', { name: 'Save mark' }).click();
    await expect(cells.nth(1)).toContainText('◐', { timeout: 30_000 });
    await expect(detail.getByText('Rig: Limited')).toBeVisible();
    await expect(detail.getByText('· high cirrus')).toBeVisible();
    const gap = timeline.locator('.nights-gap[data-reason="unavailable"]');
    await expect(gap.first()).toBeAttached({ timeout: 30_000 });
    await shot(page, 'nights-marked', name => testInfo.outputPath(name));

    // The night is planned by the rig's own runtime only.
    await expect(detail.getByText('Target Scheduler decides', { exact: true })).toBeVisible();
    await expect(detail.getByRole('button', { name: /^Compare with/ })).toHaveCount(0);

    // On a phone the grid scrolls in its own box; the page does not.
    await page.setViewportSize({ width: 390, height: 900 });
    await expect(grid).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    await shot(page, 'nights-phone', name => testInfo.outputPath(name));

    // Live says what each rig plans tonight and links to the night.
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/#/rigs');
    const tonight = page.getByTestId('director-dashboard').getByRole('link', { name: /clear/ }).first();
    await expect(tonight).toBeVisible({ timeout: 30_000 });
    await shot(page, 'rigs-live', name => testInfo.outputPath(name));
    await tonight.click();
    await expect(page.getByRole('tab', { name: 'Nights' })).toHaveAttribute('aria-selected', 'true');
    const tonightDetail = page.getByRole('region', { name: 'Night details' });
    await expect(tonightDetail).toBeVisible();
    // Tonight shows what the rig says it is doing.
    await expect(tonightDetail.getByRole('status').filter({ hasText: /^Now:/ })).toBeVisible();
    expect(errors).toEqual([]);
  } finally {
    for (const slug of slugs) await request.delete(`/api/databases/${slug}`);
  }
});
