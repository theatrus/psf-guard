import { expect, test, type APIRequestContext } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';
import { openSettings } from '../e2e/helpers';
import { shot } from './nightsFixtures';

const json = async (response: Awaited<ReturnType<APIRequestContext['post']>>) => {
  expect(response.ok(), await response.text()).toBeTruthy();
  return (await response.json()).data;
};

test('a rig applies reported equipment and preserves Director work permission beside Target Scheduler', async ({ page, request }, testInfo) => {
  test.setTimeout(180_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => void dialog.accept());
  await page.setViewportSize({ width: 1280, height: 1000 });
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'plugin-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  db.prepare('INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,?,?,?,100,30,1,0,?)').run('p', 'Ha', 'Ha', randomUUID());
  db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'p','Plugin Heart','',1,1,0,0,?)").run(randomUUID());
  db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,rotation,projectid,guid) VALUES(1,?,1,2.5466667,61.45,2,0,1,?)').run('Plugin Heart', randomUUID());
  db.prepare('INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,?,300,120,1,1,1,1,1,?)').run('p', randomUUID());
  db.prepare('INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(1,1,1,1750000000,?,1,?,?,1,?)').run('Ha', '{}', 'p', randomUUID());
  db.close();
  const slug = `plugin-${randomUUID().slice(0, 8)}`;
  await json(await request.post('/api/databases', { data: { name: 'Plugin rig', slug, db_path: dbPath, image_dirs: [root] } }));
  try {
    const catalog = randomUUID();
    const reviewed = await json(await request.post(`/api/director/v1/catalogs/${slug}/rig/preview`, { data: { catalog_id: catalog } }));
    await json(await request.post(`/api/director/v1/catalogs/${slug}/rig/apply`, { data: { plan: { catalog_id: catalog }, preview_digest: reviewed.preview_digest } }));
    const view = await json(await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`));
    const rig = view.rig.id as string;
    await json(await request.put(`/api/director/v1/catalogs/${slug}/rig/profile`, { data: {
      expected_revision: view.profile.revision,
      optics: { value: { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 749, aperture_mm: 107, rotation: { mode: 'manual', angle_degrees: 0 } }, source: { kind: 'manual' } },
      site: { value: { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 }, source: { kind: 'manual' } }, horizon: null, sky_quality: null,
      limits: { value: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, meridian_exclusion: { before_ms: 0, after_ms: 0 } }, source: { kind: 'manual' } },
      peer_id: null, mode: 'target_scheduler',
    } }));

    await page.goto('/#/?db=parked');
    await expect(page.locator('[data-project-key]').filter({ hasText: 'Plugin Heart' })).toHaveCount(1);
    await page.locator('[data-project-key]').filter({ hasText: 'Plugin Heart' }).getByRole('button', { name: /^Open the .* plan$/ }).first().click();
    await page.getByRole('button', { name: "Add a column on the mosaic's east side" }).click();
    const saveBar = page.getByRole('region', { name: 'Unsaved changes' });
    await saveBar.getByRole('button', { name: 'Save changes' }).click();
    await expect(saveBar).toContainText('All changes saved');
    await page.getByRole('region', { name: 'Needs activating' }).getByRole('button', { name: /^Activate/ }).click();
    const dialog = page.getByRole('dialog', { name: 'Activate on the rigs' });
    await expect(dialog.getByRole('status')).toHaveText('Preview ready');
    await dialog.getByRole('button', { name: 'Activate', exact: true }).click();
    await expect(dialog.getByText(/^Activated/).first()).toBeVisible();
    await page.keyboard.press('Escape');

    await openSettings(page);
    const settings = page.locator('.tauri-settings');
    await settings.getByRole('tab', { name: 'Overview', exact: true }).click();
    await settings.getByRole('button', { name: 'Open Plugin rig' }).click();
    await settings.getByRole('tab', { name: 'Rig setup', exact: true }).click();
    const sections = settings.getByRole('tablist', { name: 'Rig setup sections' });
    await sections.getByRole('tab', { name: 'Plugins', exact: true }).click();
    const panel = settings.getByRole('region', { name: 'Director plugin' });
    const plugins = settings.getByRole('group', { name: 'Plugins on this rig' });
    await expect(plugins).toContainText('Picks the work: Target Scheduler');
    await expect(plugins.getByTestId('plugin-director')).toContainText('Not paired');

    // The code comes from the page; everything the plugin does goes over HTTP.
    await panel.getByRole('button', { name: 'Generate Director pairing code' }).click();
    const code = await panel.getByLabel('Director pairing code').inputValue();
    expect(code).toMatch(/^psfdpt_[0-9a-f]{64}$/);
    await shot(panel, 'director-pairing-code', name => testInfo.outputPath(name), false);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect.poll(() => panel.evaluate(element => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(0);
    await shot(panel, 'director-pairing-phone', name => testInfo.outputPath(name), false);
    await page.setViewportSize({ width: 1280, height: 1000 });
    const profile = randomUUID();
    const paired = await json(await request.post('/api/director/v1/pair', { data: { protocol_version: 1, pairing_token: code, profile_id: profile, client_name: 'NINA observatory' } }));
    const plugin = { Authorization: `Bearer ${paired.token}`, 'X-PSF-Director-Profile': profile };
    await expect(panel.getByRole('list', { name: 'Paired Director plugins' })).toContainText('NINA observatory');
    await expect(panel.getByLabel('Director pairing code')).toHaveCount(0);

    // Trying the plugin leaves Target Scheduler running the rig.
    await expect(panel.locator('.rig-plugin-paths')).toContainText('Target Scheduler');
    const instance = paired.coordinator_instance_id as string;
    const configuration = {
      rig_id: rig, id: 'config-1', camera_id: 'camera-1', filter_wheel_id: 'wheel-1', filters: [{ id: 'filter-2', position: 2 }],
      binning_modes: [{ x: 1, y: 1 }], readout_modes: [0], gain: { support: 'range', minimum: 0, maximum: 200 },
      offset: { support: 'range', minimum: 0, maximum: 100 }, exposure_min_ms: 1, exposure_max_ms: 600000, enable_slew_center: true, dither_every: 0,
    };
    await json(await request.post(`/api/director/v1/rigs/${rig}/equipment-reports`, { headers: plugin, data: {
      coordinator_instance_id: instance, catalog_id: catalog, report_id: randomUUID(), observed_at_ms: Date.now(), configuration, filter_names: { 'filter-2': 'Ha' },
    } }));
    const equipment = panel.getByRole('group', { name: 'Equipment', exact: true });
    await expect(equipment).toContainText('Accepted: camera-1', { timeout: 20_000 });
    await expect(equipment).toContainText('equipment is up to date');
    await expect(equipment.getByRole('button', { name: 'Accept equipment' })).toHaveCount(0);

    const allow = panel.getByRole('group', { name: 'Allow work', exact: true });
    const work = allow.getByTestId('rig-plugin-work');
    await expect(work).toHaveAttribute('data-state', 'ready');
    await expect(allow.getByTestId('rig-plugin-plans')).toContainText('Plugin Heart');
    // An explicit off setting survives an otherwise identical equipment report.
    await allow.getByRole('button', { name: 'Turn off', exact: true }).click();
    await expect(work).toHaveAttribute('data-state', 'off');
    await json(await request.post(`/api/director/v1/rigs/${rig}/equipment-reports`, { headers: plugin, data: {
      coordinator_instance_id: instance, catalog_id: catalog, report_id: randomUUID(), observed_at_ms: Date.now(), configuration, filter_names: { 'filter-2': 'Ha' },
    } }));
    await panel.getByRole('button', { name: 'Reload Director plugin' }).click();
    await expect(work).toHaveAttribute('data-state', 'off');
    await allow.getByRole('button', { name: 'Allow work again' }).click();
    await expect(work).toHaveAttribute('data-state', 'ready');

    // Switching paths to compare them preserves work permission.
    const mode = settings.getByRole('group', { name: 'Mode' });
    for (const next of ['Director', 'Target Scheduler']) {
      await panel.getByRole('button', { name: 'Change mode' }).click();
      await mode.getByRole('radio', { name: next }).check();
      await settings.getByRole('button', { name: 'Save rig profile' }).click();
      await expect(settings.getByRole('status').filter({ hasText: /Saved rig profile revision/ })).toBeVisible();
      await sections.getByRole('tab', { name: 'Plugins', exact: true }).click();
      await expect(panel.locator('.rig-plugin-paths')).toContainText(next);
      await expect(work).toHaveAttribute('data-state', 'ready');
    }

    // A real setup change also preserves work permission.
    await sections.getByRole('tab', { name: 'Optics', exact: true }).click();
    await settings.getByLabel('Focal length').fill('750');
    await settings.getByRole('button', { name: 'Save rig profile' }).click();
    await expect(settings.getByRole('status').filter({ hasText: /Saved rig profile revision/ })).toBeVisible();
    await sections.getByRole('tab', { name: 'Plugins', exact: true }).click();
    await expect(work).toHaveAttribute('data-state', 'ready');

    // The plugin gets work under that policy, then is removed while holding it.
    const requested = await json(await request.post(`/api/director/v1/rigs/${rig}/workloads/request`, { headers: plugin, data: {
      coordinator_instance_id: instance, catalog_id: catalog, request_id: randomUUID(), configuration_id: 'config-1', execution_mode: 'native_imaging_v1',
    } }));
    expect(requested.state).toBe('issued');
    const running = panel.getByRole('group', { name: 'Work', exact: true });
    await expect(running).toContainText('Not started. Issued', { timeout: 20_000 });
    await expect(running.getByRole('button', { name: 'Close work' })).toHaveCount(0);
    await shot(page, 'director-in-settings', name => testInfo.outputPath(name), false);
    await shot(panel, 'director-lifecycle', name => testInfo.outputPath(name), false);

    await panel.getByRole('button', { name: 'Remove NINA observatory' }).click();
    await expect(running.getByRole('alert')).toContainText('this rig cannot take new work');
    await expect(work).toHaveAttribute('data-state', 'plugin_removed');
    await shot(running, 'director-stranded-work', name => testInfo.outputPath(name), false);
    await running.getByRole('button', { name: 'Close work' }).click();
    await expect(running).toContainText('No open work.');
    await expect(running).toContainText('Previous work retired');

    // Both plugins pair on the Plugins tab, and Sync points there.
    await expect(settings.getByRole('heading', { name: 'Sync plugin', exact: true })).toBeVisible();
    await expect(plugins.getByTestId('plugin-sync')).toBeVisible();
    await settings.getByRole('tab', { name: 'Sync', exact: true }).click();
    await expect(settings.getByRole('heading', { name: 'PSF Guard Sync plugin' })).toBeVisible();
    await expect(settings.getByRole('button', { name: 'Open Rig setup' })).toBeVisible();
    expect(errors).toEqual([]);
  } finally {
    await request.delete(`/api/databases/${slug}`).catch(() => undefined);
  }
});
