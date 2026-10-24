import { expect, test } from '@playwright/test';
import * as path from 'path';
import { openSettings, registerFixtureDb, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

test('WBPP has a dedicated folder, percentage cap and optional forever masters', async ({
  page,
  request,
}) => {
  await resetDatabases(request);
  const db = await registerFixtureDb(request);
  await waitForCacheReady(request, db.id);
  const original = (await (await request.get('/api/settings/pixinsight')).json()).data;
  const storage = (await (await request.get('/api/settings/storage')).json()).data;
  try {
    await page.goto('/');
    await openSettings(page);
    await page.getByRole('tab', { name: 'Storage', exact: true }).click();
    const folder = page.getByLabel('WBPP storage folder');
    await folder.fill(path.join(tmpBase(), 'wbpp-runs'));
    const limit = page.getByRole('slider', { name: 'WBPP volume usage limit, in percent' });
    await expect(limit).toHaveAttribute('min', '0');
    await limit.fill('25');
    await page.getByLabel('WBPP intermediate retention, in days').fill('14');
    const masterDays = page.getByLabel('WBPP master retention, in days');
    await masterDays.fill('');
    await expect(page.getByRole('checkbox', { name: /^Keep WBPP masters forever/ })).not.toBeChecked();
    await masterDays.fill('120');
    await page.getByRole('checkbox', { name: /^Keep WBPP masters forever/ }).check();
    await expect(page.getByLabel('WBPP master retention, in days')).toBeDisabled();
    await page.getByRole('button', { name: 'Save WBPP storage', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Save WBPP storage', exact: true })).toBeDisabled();
    const saved = (await (await request.get('/api/settings/pixinsight')).json()).data;
    expect(saved.runs_dir).toBe(path.join(tmpBase(), 'wbpp-runs'));
    expect(saved.storage).toEqual({ max_volume_percent: 25, intermediate_days: 14, master_days: 0 });
    const general = page.getByRole('slider', { name: /^Volume usage limit, in percent$/ });
    await expect(general).toHaveAttribute('min', '0');
    await general.fill('20');
    await general.press('Enter');
    await expect
      .poll(async () => (await (await request.get('/api/settings/storage')).json()).data.max_volume_percent)
      .toBe(20);
  } finally {
    await request.put('/api/settings/pixinsight', {
      data: { binary: original.binary, runs_dir: original.runs_dir, storage: original.storage },
    });
    await request.put('/api/settings/storage', { data: { max_volume_percent: storage.max_volume_percent } });
  }
});
