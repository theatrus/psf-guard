import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { addRig, rigDatabase, shot } from './nightsFixtures';

test('a Target Scheduler rig\'s night plans its changed settings before they are saved to its database', async ({ page, request }, testInfo) => {
  test.setTimeout(120_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const slug = `ts-settings-${randomUUID().slice(0, 6)}`;
  const dbPath = rigDatabase([
    { name: 'Heart Nebula', priority: 2, target: 'IC 1805', ra_hours: 2.5467, dec: 61.45, filters: [['Ha', 120]] },
    { name: 'Soul Nebula', priority: 0, target: 'IC 1848', ra_hours: 2.8533, dec: 60.4, filters: [['Ha', 120]] },
  ]);
  try {
    await addRig(request, 'Observatory rig', slug, dbPath, { latitude_degrees: 34.2, longitude_degrees: -118.3 });
    await page.setViewportSize({ width: 1440, height: 1100 });
    // Ten days on, so the night is always ahead.
    const ahead = new Date(Date.now() + 10 * 86_400_000);
    const from = `${ahead.getFullYear()}-${String(ahead.getMonth() + 1).padStart(2, '0')}-${String(ahead.getDate()).padStart(2, '0')}`;
    await page.goto(`/#/rigs?rigsTab=nights&nightsFrom=${from}`);
    const grid = page.getByTestId('nights-grid');
    await expect(grid).toBeVisible({ timeout: 30_000 });
    await grid.getByRole('row').filter({ hasText: 'Observatory rig' }).getByRole('button').first().click();
    const detail = page.getByRole('region', { name: 'Night details' });
    await expect(detail.getByText('Target Scheduler decides', { exact: true })).toBeVisible();
    const timeline = detail.getByRole('group', { name: 'Schedule' });
    const blocks = timeline.locator('.nights-block');
    await expect(blocks.first()).toBeVisible({ timeout: 30_000 });
    // Heart Nebula's high priority takes the night.
    await expect(blocks.first()).toHaveAccessibleName(/^Heart Nebula, IC 1805/);

    const settings = detail.getByRole('region', { name: 'Target Scheduler settings' });
    // Folded until asked for.
    await expect(settings).toContainText('2 of 2 projects active');
    await expect(settings.getByLabel('Heart Nebula active')).toHaveCount(0);
    await settings.getByRole('button', { name: 'Target Scheduler settings' }).click();
    await expect(settings).toContainText('Changes apply to every night, not just this one.');
    await settings.getByRole('button', { name: 'Change settings' }).click();
    await settings.getByLabel('Heart Nebula active').uncheck();
    await settings.getByLabel('Soul Nebula priority').selectOption('high');
    // The schedule plans the change before it is saved: Soul Nebula now.
    await expect(blocks.first()).toHaveAccessibleName(/^Soul Nebula, IC 1848/, { timeout: 30_000 });
    const bar = detail.getByRole('region', { name: 'Unsaved changes' });
    await expect(bar).toContainText('Target Scheduler settings: Heart Nebula: inactive; Soul Nebula: priority High');
    const db = new Database(dbPath, { readonly: true });
    const project = (name: string) => db.prepare('SELECT priority, state FROM project WHERE name = ?').get(name) as { priority: number; state: number };
    expect(project('Heart Nebula')).toEqual({ priority: 2, state: 1 });
    await page.setViewportSize({ width: 1440, height: 2400 });
    await shot(detail, 'nights-ts-settings', name => testInfo.outputPath(name), false);
    await page.setViewportSize({ width: 1440, height: 1100 });

    await bar.getByRole('button', { name: 'Save changes' }).click();
    await expect(settings.getByRole('button', { name: 'Change settings' })).toBeVisible();
    expect(project('Heart Nebula')).toEqual({ priority: 2, state: 2 });
    expect(project('Soul Nebula')).toEqual({ priority: 2, state: 1 });
    db.close();
    expect(errors).toEqual([]);
  } finally {
    await request.delete(`/api/databases/${slug}`);
  }
});
