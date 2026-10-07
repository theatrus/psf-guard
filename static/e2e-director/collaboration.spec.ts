import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

test('rig setup reviews remote work and saved contributions before importing or replaying', async ({ page, request }, testInfo) => {
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'collaboration-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'profile','M31','',1,1,0,0,?)").run(randomUUID());
  db.close();
  const slug = `collaboration-${randomUUID().slice(0, 8)}`;
  const added = await request.post('/api/databases', { data: { name: 'Collaboration rig', slug, db_path: dbPath, image_dirs: [root] } });
  expect(added.ok(), await added.text()).toBeTruthy();
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const connection = randomUUID();
  const operations: Record<string, unknown>[] = [];
  let deliveries = 0;
  const share = { task_id: '000000000004', name: 'M31 collaboration', version: 2, review_reasons: [], demands: [{ panel_index: 0, filter: 'H', exposure_ms: 300000, requested_frames: 12 }] };
  await page.route('**/api/director/v1/rigs/*/collaboration', route => route.fulfill({ json: { success: true, data: [{ status: 'registered', binding: {
    id: connection, rig_id: randomUUID(), name: 'Collaboration rig', base_url: 'https://collab.example/', agent_id: '000000000001', state: 'registered', allow_loopback_http: false,
    settings: { binning: 1, colour: false, hours_per_night: 6, share_status: false, filters: { Ha: { exposure_seconds: 300, bandpass_nm: 7 } } },
  } }] } }));
  await page.route(`**/api/director/v1/collaboration/${connection}/work`, async route => {
    const input = route.request().postDataJSON();
    operations.push(input);
    let data: unknown = {};
    switch (input.operation) {
      case 'tonight': data = { shares: [share] }; break;
      case 'preview': data = { preview: { review_digest: 'work-digest', acquisition_enabled: false }, plan: { project_id: randomUUID(), share } }; break;
      case 'report_inputs': data = { imports: [{ id: 'import', name: share.name, night: '2026-10-05', panels: [0] }], catalogs: [{ id: slug, name: 'Collaboration rig' }] }; break;
      case 'report_candidates': data = { images: [{ guid: 'image-guid', file: 'M31-Ha-001.fits', filter: 'Ha', target: 'M31', captured_at: 1791171000 }] }; break;
      case 'preview_report': data = { review_digest: 'image-digest', report: { frames: 1, seconds: 300, filterName: 'H', calibrated: false, footprint: { width: 1, height: 1 } } }; break;
      case 'checkin':
        if (++deliveries === 1) {
          await route.fulfill({ status: 502, json: { success: false, error: 'Server offline; queued reports retained' } }); return;
        }
        data = { delivered: 1, accepted: 1, rejected: 0 }; break;
    }
    await route.fulfill({ json: { success: true, data } });
  });
  try {
    await page.goto('/#/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.locator('.tauri-settings');
    await settings.getByRole('tab', { name: 'Rigs' }).click();
    await settings.getByRole('button', { name: 'Setup Collaboration rig' }).click();
    const collaboration = settings.getByRole('region', { name: 'Collaboration', exact: true });
    await expect(collaboration.getByRole('button', { name: 'Pull nightly work' })).toBeDisabled();
    await collaboration.getByLabel('Night', { exact: true }).fill('2026-10-05');
    await collaboration.getByLabel('Moon illumination (%)').fill('12');
    await collaboration.getByLabel('Moon above horizon (%)').fill('30');
    await collaboration.getByRole('button', { name: 'Pull nightly work' }).click();
    await collaboration.getByRole('button', { name: 'Review import' }).click();
    await expect(collaboration.getByRole('region', { name: 'Review collaboration import' })).toContainText('12');
    expect(operations.some(o => o.operation === 'apply')).toBe(false);
    await collaboration.getByRole('button', { name: 'Import draft' }).click();
    await expect(collaboration).toContainText('Imported as an inactive project draft');
    expect(operations.find(o => o.operation === 'apply')).toMatchObject({ task: share.task_id, review_digest: 'work-digest' });

    await collaboration.getByRole('button', { name: 'Contribution reports' }).click();
    await collaboration.getByLabel('Imported visit').selectOption({ label: 'M31 collaboration (2026-10-05)' });
    await collaboration.getByLabel('Rig database').selectOption(slug);
    await collaboration.getByLabel('Remote panel').selectOption('0');
    await collaboration.getByLabel('Select M31-Ha-001.fits').check();
    await collaboration.getByRole('button', { name: 'Review 1 images' }).click();
    await expect(collaboration).toContainText('Measured shared coverage: 1.000 x 1.000 degrees');
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      await collaboration.getByRole('button', { name: 'Contribution reports' }).scrollIntoViewIfNeeded();
      expect(await collaboration.evaluate(e => e.scrollWidth <= e.clientWidth)).toBeTruthy();
      await page.screenshot({ path: testInfo.outputPath(`collaboration-${width}.png`) });
    }
    await collaboration.getByRole('button', { name: 'Queue finalized contribution' }).click();
    await expect(collaboration).toContainText('Contribution queued for check-in');
    expect(deliveries).toBe(0);
    await collaboration.getByRole('button', { name: 'Check in', exact: true }).click();
    await expect(collaboration.getByRole('alert')).toContainText('queued reports retained');
    expect(deliveries).toBe(1);
    await collaboration.getByRole('button', { name: 'Check in', exact: true }).click();
    await expect(collaboration).toContainText('1 reports delivered; 1 accepted; 0 rejected');
    expect(deliveries).toBe(2);
    expect(errors).toEqual([]);
  } finally {
    const removed = await request.delete(`/api/databases/${slug}`);
    expect(removed.ok(), await removed.text()).toBeTruthy();
  }
});
