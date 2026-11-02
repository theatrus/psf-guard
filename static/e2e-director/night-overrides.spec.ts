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

test('a Director rig\'s night takes overrides, planned before they are saved', async ({ page, request }, testInfo) => {
  test.setTimeout(150_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 1440, height: 1100 });
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'overrides-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  db.prepare('INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,?,?,?,100,30,1,0,?)').run('p', 'Ha', 'Ha', randomUUID());
  db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'p','Heart Nebula','',1,1,0,0,?)").run(randomUUID());
  db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,rotation,projectid,guid) VALUES(1,?,1,2.5466667,61.45,2,0,1,?)').run('Heart Nebula', randomUUID());
  db.prepare('INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,?,300,120,1,1,1,1,1,?)').run('p', randomUUID());
  // One frame shot, so the project lists among those with images.
  db.prepare('INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(1,1,1,1750000000,?,1,?,?,1,?)').run('Ha', JSON.stringify({ FileName: 'heart.fits' }), 'p', randomUUID());
  db.close();
  const slug = `overrides-${randomUUID().slice(0, 8)}`;
  await json(await request.post('/api/databases', { data: { name: 'Director rig', slug, db_path: dbPath, image_dirs: [root] } }));
  try {
    // The rig, its optics, and a Los Angeles location with a 25° minimum.
    const catalog = randomUUID();
    const reviewed = await json(await request.post(`/api/director/v1/catalogs/${slug}/rig/preview`, { data: { catalog_id: catalog } }));
    await json(await request.post(`/api/director/v1/catalogs/${slug}/rig/apply`, { data: { plan: { catalog_id: catalog }, preview_digest: reviewed.preview_digest } }));
    const view = await json(await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`));
    const rig = view.rig.id as string;
    const site = { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 };
    await json(await request.put(`/api/director/v1/catalogs/${slug}/rig/profile`, { data: {
      expected_revision: view.profile.revision,
      optics: { value: { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 749, aperture_mm: 107, rotation: { mode: 'manual', angle_degrees: 0 } }, source: { kind: 'manual' } },
      site: { value: site, source: { kind: 'manual' } }, horizon: null, sky_quality: null,
      limits: { value: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, meridian_exclusion: { before_ms: 0, after_ms: 0 } }, source: { kind: 'manual' } },
      peer_id: null,
    } }));

    // A two-panel Director plan, activated on the rig.
    await page.goto('/#/?db=parked');
    await expect(page.locator('[data-project-key]').filter({ hasText: 'Heart Nebula' })).toHaveCount(1);
    await page.locator('[data-project-key]').filter({ hasText: 'Heart Nebula' }).getByRole('button', { name: /^Open the .* plan$/ }).first().click();
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

    // Director drives the rig: a paired plugin reports the equipment.
    const instance = (await json(await request.get('/api/director/v1/status'))).instance_id as string;
    const profileId = randomUUID();
    const token = await json(await request.post(`/api/director/v1/rigs/${rig}/pairing-token`, { data: { coordinator_instance_id: instance, catalog_id: catalog } }));
    await json(await request.post('/api/director/v1/pair', { data: { protocol_version: 1, pairing_token: token.pairing_token, profile_id: profileId, client_name: 'NINA' } }));
    const configuration = {
      rig_id: rig, id: 'config-1', camera_id: 'camera-1', filter_wheel_id: 'wheel-1', filters: [{ id: 'filter-2', position: 2 }],
      binning_modes: [{ x: 1, y: 1 }], readout_modes: [0], gain: { support: 'range', minimum: 0, maximum: 200 },
      offset: { support: 'range', minimum: 0, maximum: 100 }, exposure_min_ms: 1, exposure_max_ms: 600000, enable_slew_center: true, dither_every: 0,
    };
    await json(await request.put(`/api/director/v1/rigs/${rig}/equipment`, { data: {
      coordinator_instance_id: instance, catalog_id: catalog, configuration, filter_names: { 'filter-2': 'Ha' },
      optics: { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 749, aperture_mm: 107, rotation: { mode: 'rotator' } },
      site, horizon: null, limits: null, reported_at_ms: Date.now(),
    } }));
    // Pairing a Director plugin set the rig's mode.
    const reported = await json(await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`));
    expect(reported.mode).toEqual({ set: 'director', runtime: 'director', paired: true, equipment: true });
    // Rig setup shows the mode and what Director needs there.
    await openSettings(page);
    const settingsPage = page.locator('.tauri-settings');
    await settingsPage.getByRole('tab', { name: 'Overview', exact: true }).click();
    await settingsPage.getByRole('button', { name: 'Open Director rig' }).click();
    await settingsPage.getByRole('tab', { name: 'Rig setup', exact: true }).click();
    const mode = settingsPage.getByRole('group', { name: 'Mode' });
    await expect(mode.getByRole('radio', { name: 'Director' })).toBeChecked();
    await expect(mode.getByRole('list', { name: 'What Director needs' })).toContainText('The N.I.N.A. Director plugin is paired with this rig.');
    await expect(mode).not.toContainText('Not set yet');
    await shot(mode, 'rig-mode', name => testInfo.outputPath(name), false);
    await settingsPage.getByRole('button', { name: 'Close settings' }).click();

    // Ten days on, so the night is always ahead.
    const ahead = new Date(Date.now() + 10 * 86_400_000);
    const from = `${ahead.getFullYear()}-${String(ahead.getMonth() + 1).padStart(2, '0')}-${String(ahead.getDate()).padStart(2, '0')}`;
    await page.goto(`/#/rigs?rigsTab=nights&nightsFrom=${from}`);
    const grid = page.getByTestId('nights-grid');
    await expect(grid).toBeVisible({ timeout: 30_000 });
    const row = grid.getByRole('row').filter({ hasText: 'Director rig' });
    await expect(row).toContainText('Director');
    await row.getByRole('button').first().click();
    const detail = page.getByRole('region', { name: 'Night details' });
    await expect(detail.getByText('Director decides', { exact: true })).toBeVisible();
    await expect(detail.getByRole('heading', { name: 'Director schedule' })).toBeVisible();
    const timeline = detail.getByRole('group', { name: 'Schedule' });
    const blocks = timeline.locator('.nights-block');
    await expect(blocks.first()).toBeVisible({ timeout: 30_000 });

    const overrides = detail.getByRole('region', { name: 'Overrides' });
    await expect(overrides.getByText('None: the rig follows its own program this night.')).toBeVisible();
    await detail.getByRole('button', { name: 'Edit night' }).click();

    // Drag the first block an hour later: its target is pinned there.
    const track = await timeline.locator('.nights-timeline-track').boundingBox();
    const first = await blocks.first().boundingBox();
    expect(track && first).toBeTruthy();
    // The track runs from an hour before dusk to an hour after dawn, about
    // thirteen hours in November.
    const hourPx = track!.width / 13;
    await page.mouse.move(first!.x + first!.width / 2, first!.y + first!.height / 2);
    await page.mouse.down();
    await page.mouse.move(first!.x + first!.width / 2 + hourPx / 2, first!.y + first!.height / 2, { steps: 4 });
    await page.mouse.move(first!.x + first!.width / 2 + hourPx, first!.y + first!.height / 2, { steps: 4 });
    await page.mouse.up();
    const list = overrides.getByRole('list');
    await expect(list.getByRole('listitem').filter({ hasText: /^Only Heart Nebula/ })).toHaveCount(1);
    await expect(list.getByRole('listitem').filter({ hasText: /^Leave out Heart Nebula/ })).toHaveCount(1);
    await expect(detail.getByText('Planning the edit...')).toHaveCount(0);

    // Hold the rig two hours before dawn, by the hold line.
    const hold = detail.getByRole('slider', { name: 'Hold the rig from' });
    await expect(hold).toHaveAttribute('aria-valuetext', 'No hold');
    await hold.focus();
    for (let step = 0; step < 8; step += 1) await hold.press('ArrowLeft');
    await expect(list.getByRole('listitem').filter({ hasText: /^Hold the rig from/ })).toHaveCount(1);
    // The schedule shows the edit: the pin on the timeline, and the night
    // held to dawn.
    const bands = timeline.getByRole('group', { name: 'Overrides' });
    await expect(bands.getByRole('button', { name: /^Only Heart Nebula/ })).toBeVisible();
    await expect(detail.getByText('Planning the edit...')).toHaveCount(0);
    await expect(detail.getByRole('list', { name: 'Legend' })).toContainText("Held: The night's overrides hold the rig.");
    const bar = detail.getByRole('region', { name: 'Unsaved changes' });
    await expect(bar).toContainText("This night's overrides:");
    // Tall enough that the panel fits without scrolling under the header.
    await page.setViewportSize({ width: 1440, height: 2600 });
    await shot(detail, 'nights-overrides', name => testInfo.outputPath(name), false);
    await page.setViewportSize({ width: 1440, height: 1100 });

    await bar.getByRole('button', { name: 'Save changes' }).click();
    await expect(detail.getByRole('button', { name: 'Edit night' })).toBeVisible();
    const saved = await json(await request.get(`/api/director/v1/rigs/${rig}/night-overrides/${from}`));
    expect(saved.entries.map((entry: { kind: string }) => entry.kind).sort()).toEqual(['exclude', 'hold', 'pin']);
    await expect(row.getByRole('button').first()).toHaveAccessibleName(/, 3 overrides$/);
    expect(errors).toEqual([]);
  } finally {
    await request.delete(`/api/databases/${slug}`);
  }
});
