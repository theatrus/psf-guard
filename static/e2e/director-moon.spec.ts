import { randomUUID } from 'node:crypto';
import { expect, test } from '@playwright/test';

test('exposure Moon policy saves, survives reload and retains disabled values', async ({ page, request }, testInfo) => {
  const id = randomUUID();
  const name = `Moon fixture ${id.slice(0, 8)}`;
  const created = await request.put(`/api/director/v1/templates/${id}`, { data: {
    id, revision: 0, name, filter_name: 'OIII', gain: null, offset: null,
    bin: 1, readout_mode: null, default_exposure_seconds: 120, updated_at_ms: 0,
  } });
  expect(created.ok()).toBeTruthy();
  const open = async () => {
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('tab', { name: 'Exposure templates', exact: true }).click();
    await page.getByRole('button', { name: `Moon settings for ${name}`, exact: true }).click();
    return page.getByRole('group', { name: 'Moon avoidance', exact: true });
  };
  try {
    await page.goto('/');
    let group = await open();
    await group.getByRole('checkbox', { name: 'Enable Moon avoidance' }).check();
    await group.getByLabel('Separation at full Moon').fill('82.5');
    await group.getByLabel('Half-separation width').fill('0');
    await expect(page.getByRole('button', { name: `Save ${name}`, exact: true })).toBeDisabled();
    await group.getByLabel('Half-separation width').fill('9');
    await group.getByRole('checkbox', { name: 'Moon must be down' }).check();
    await page.getByRole('button', { name: `Save ${name}`, exact: true }).click();
    await expect(page.getByText(`Saved ${name}.`, { exact: true })).toBeVisible();
    await page.reload();
    group = await open();
    await expect(group.getByLabel('Separation at full Moon')).toHaveValue('82.5');
    await expect(group.getByRole('checkbox', { name: 'Moon must be down' })).toBeChecked();
    await page.screenshot({ path: testInfo.outputPath('moon-desktop.png') });
    await page.setViewportSize({ width: 390, height: 844 });
    await group.scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath('moon-mobile.png') });
    await group.getByRole('checkbox', { name: 'Enable Moon avoidance' }).uncheck();
    await page.getByRole('button', { name: `Save ${name}`, exact: true }).click();
    await expect(page.getByText(`Saved ${name}.`, { exact: true })).toBeVisible();
    await expect.poll(async () => {
      const all = (await (await request.get('/api/director/v1/templates')).json()).data;
      return all.find((template: { id: string }) => template.id === id)?.moon;
    }).toMatchObject({ enabled: false, separation_degrees: 82.5, width_days: 9, moon_down: true });
  } finally {
    const all = (await (await request.get('/api/director/v1/templates')).json()).data;
    const saved = all.find((template: { id: string }) => template.id === id);
    if (saved) expect((await request.delete(`/api/director/v1/templates/${id}?revision=${saved.revision}`)).ok()).toBeTruthy();
  }
});
