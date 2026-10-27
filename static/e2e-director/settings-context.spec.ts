import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';
import { openSettings, selectSettingsSection } from '../e2e/helpers';

test('rig setup shares the opened database context with files and sync', async ({ page, request }, testInfo) => {
  const slugs: string[] = [];
  const post = async (route: string, data: unknown) => {
    const response = await request.post(`/api${route}`, { data });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  try {
    for (const name of ['C925', 'Ultra Cat']) {
      const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'settings-context-'));
      const file = path.join(root, 'catalog.sqlite');
      const db = new Database(file);
      applyRealSchema(db);
      db.close();
      const slug = `context-${randomUUID().slice(0, 8)}`;
      await post('/databases', { name, slug, db_path: file, image_dirs: [root] });
      slugs.push(slug);
      const catalog = randomUUID();
      const preview = await post(`/director/v1/catalogs/${slug}/rig/preview`, { catalog_id: catalog });
      await post(`/director/v1/catalogs/${slug}/rig/apply`, { plan: { catalog_id: catalog }, preview_digest: preview.preview_digest });
      const loaded = (await (await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`)).json()).data;
      const saved = await request.put(`/api/director/v1/catalogs/${slug}/rig/profile`, { data: {
        expected_revision: loaded.profile.revision,
        optics: { value: { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76,
          focal_length_mm: name === 'C925' ? 1645 : 250, aperture_mm: name === 'C925' ? 235 : 51,
          rotation: { mode: 'manual', angle_degrees: 0 } }, source: { kind: 'manual' } },
        site: null, horizon: null, sky_quality: null, peer_id: null,
        limits: { value: loaded.profile.limits.value, source: { kind: 'manual' } },
      } });
      expect(saved.ok(), await saved.text()).toBe(true);
    }
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/#/sky');
    await openSettings(page);
    await page.getByRole('button', { name: 'Open C925', exact: true }).click();
    await selectSettingsSection(page, 'rigs');
    await expect(page.getByRole('region', { name: 'Rig profile', exact: true })).toBeVisible();
    await expect(page.getByLabel('Focal length', { exact: true })).toHaveValue('1645');
    await page.screenshot({ path: testInfo.outputPath('rig-setup-desktop.png'), animations: 'disabled' });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({ path: testInfo.outputPath('rig-setup-phone.png'), animations: 'disabled' });
    expect(await page.locator('.tauri-settings').evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await page.getByLabel('Focal length', { exact: true }).fill('1500');
    await selectSettingsSection(page, 'databases');
    await expect(page.getByRole('alertdialog')).toContainText('C925 setup');
    await page.getByRole('button', { name: 'Keep editing' }).click();
    await expect(page.getByLabel('Focal length', { exact: true })).toHaveValue('1500');
    await page.getByRole('button', { name: 'Databases & rigs', exact: true }).click();
    await page.getByRole('button', { name: 'Discard changes and switch' }).click();
    await page.getByRole('button', { name: 'Open Ultra Cat', exact: true }).click();
    await selectSettingsSection(page, 'rigs');
    await expect(page.getByLabel('Focal length', { exact: true })).toHaveValue('250');
    await selectSettingsSection(page, 'sync');
    await expect(page.locator('.scheduler-sync-workspace').getByLabel('Transfer destination')).toHaveValue(slugs[1]);
    await selectSettingsSection(page, 'sites');
    await expect(page.getByRole('region', { name: 'Sites', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Sites', exact: true }).getByText(/Loading sites/)).toBeHidden();
    await page.screenshot({ path: testInfo.outputPath('shared-sites-phone.png'), animations: 'disabled' });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.screenshot({ path: testInfo.outputPath('shared-sites-desktop.png'), animations: 'disabled' });
    await page.getByRole('button', { name: 'Close settings' }).click();
  } finally {
    for (const slug of slugs) await request.delete(`/api/databases/${slug}`);
  }
});
