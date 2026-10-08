import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

test('pull tonight, review and activate allowed work in the background', async ({ page, request }, testInfo) => {
  const root = path.resolve('..');
  const run = process.env.PSF_GUARD_DIRECTOR_E2E_TMP!;
  const dir = fs.mkdtempSync(path.join(run, 'tonight-'));
  const slug = `collaboration-${randomUUID()}`;
  const database = new Database(path.join(dir, 'rig.sqlite'));
  applyRealSchema(database);
  database.prepare(`INSERT INTO exposuretemplate (profileId,name,filtername,gain,offset,bin,readoutmode,twilightlevel,moonavoidanceenabled,moonavoidanceseparation,moonavoidancewidth,maximumhumidity,defaultexposure,moonrelaxscale,moonrelaxmaxaltitude,moonrelaxminaltitude,moondownenabled,ditherevery,minutesOffset,guid)
    VALUES ('profile','O 300','OIII',100,30,1,-1,0,1,60,7,0,300,0,5,-15,0,3,0,?)`).run(randomUUID());
  const wire = JSON.parse(fs.readFileSync(path.join(root, 'crates/director-interop/tests/fixtures/starfront-tonight.json'), 'utf8'));
  delete wire.task;
  const remote = createServer((req, res) => {
    req.resume();
    const url = new URL(req.url!, 'http://localhost');
    res.setHeader('content-type', 'application/json');
    if (url.pathname.endsWith('/health')) res.end(JSON.stringify({ ok: true, protocol: 1, version: 'test', time: 1791171023, features: ['pairing'] }));
    else if (url.pathname.endsWith('/pair')) res.end(JSON.stringify({ agent: { id: '000000000001' }, token: 'local-test-token' }));
    else if (url.pathname.endsWith('/hello')) res.end(JSON.stringify({ agent: '000000000001', protocol: 1, serverTime: 1791171023 }));
    else if (url.pathname.endsWith('/task')) {
      const result = structuredClone(wire);
      result.tasks.forEach((task: Record<string, unknown>) => { task.assignedNight = url.searchParams.get('night'); });
      res.end(JSON.stringify(result));
    } else { res.statusCode = 404; res.end('{}'); }
  });
  await new Promise<void>(resolve => remote.listen(0, '127.0.0.1', resolve));
  const address = remote.address();
  if (!address || typeof address === 'string') throw new Error('No local fixture address');
  const post = async (route: string, data: unknown) => {
    const response = await request.post(`/api${route}`, { data });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  let connection: string | undefined;
  try {
    await post('/databases', { name: 'Collaboration test rig', slug, db_path: path.join(dir, 'rig.sqlite'), image_dirs: [dir] });
    const catalog = randomUUID();
    const reviewed = await post(`/director/v1/catalogs/${slug}/rig/preview`, { catalog_id: catalog });
    const adopted = await post(`/director/v1/catalogs/${slug}/rig/apply`, { plan: { catalog_id: catalog }, preview_digest: reviewed.preview_digest });
    const rig = adopted.binding.rig.id;
    const profileResponse = await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`);
    const profile = (await profileResponse.json()).data.profile;
    const saved = await request.put(`/api/director/v1/catalogs/${slug}/rig/profile`, { data: {
      expected_revision: profile.revision,
      optics: { value: { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 530, aperture_mm: null, rotation: { mode: 'rotator' } }, source: { kind: 'manual' } },
      site: { value: { latitude_degrees: 35, longitude_degrees: -105, elevation_meters: 2000 }, source: { kind: 'manual' } },
      horizon: null, sky_quality: null, limits: { value: profile.limits.value, source: { kind: 'manual' } }, peer_id: null,
    } });
    expect(saved.ok(), await saved.text()).toBe(true);
    const candidate = randomUUID();
    await post(`/director/v1/rigs/${rig}/collaboration`, { id: candidate, server_url: `http://127.0.0.1:${address.port}/`, name: 'Local test only', allow_loopback_http: true });
    connection = candidate;
    await post(`/director/v1/collaboration/${connection}/pair`, { code: 'LOCAL' });
    const work = `/director/v1/collaboration/${connection}/work`;
    await post(work, { operation: 'configure', settings: { binning: 1, colour: false, hours_per_night: 6, share_status: false, filters: { OIII: { exposure_seconds: 300, bandpass_nm: 7 } } } });
    await page.goto('/#/sky');
    await page.getByTitle('Settings', { exact: true }).click();
    await page.getByRole('tab', { name: 'Rigs', exact: true }).click();
    await page.getByRole('button', { name: 'Setup Collaboration test rig', exact: true }).click();
    const panel = page.getByRole('region', { name: 'Collaboration', exact: true });
    await expect(panel.getByRole('button', { name: "Pull tonight's work" })).toBeEnabled();
    const pull = page.waitForResponse(response => response.url().endsWith(`${connection}/work`) && response.request().postDataJSON().operation === 'tonight');
    await panel.getByRole('button', { name: "Pull tonight's work" }).click();
    const response = await pull;
    expect(response.request().postDataJSON()).toEqual({ operation: 'tonight' });
    const night = (await response.json()).data.night.night;
    await expect(panel).toContainText(`Observing night: ${night}`);
    await panel.getByRole('button', { name: 'Review import', exact: true }).click();
    await panel.getByRole('button', { name: 'Import draft', exact: true }).click();
    await expect(panel).toContainText('Imported as an inactive project draft');
    expect(database.prepare('SELECT COUNT(*) AS count FROM exposureplan').get()).toEqual({ count: 0 });
    await panel.getByText('Automatic work requests', { exact: true }).click();
    await panel.getByLabel('Pull tonight automatically').check();
    await panel.getByLabel('M31 halo in narrowband', { exact: true }).check();
    await panel.getByLabel('Activate in rig database').check();
    await panel.getByRole('button', { name: 'Save automation' }).click();
    await expect(panel.getByRole('button', { name: 'Refresh now' })).toBeEnabled();
    await panel.getByRole('button', { name: 'Refresh now' }).click();
    await expect(panel).toContainText('1 activated');
    expect(database.prepare('SELECT COUNT(*) AS count, MIN(desired) AS desired FROM exposureplan').get()).toEqual({ count: 6, desired: 11 });
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      await panel.getByText('Automatic work requests', { exact: true }).scrollIntoViewIfNeeded();
      await page.screenshot({ path: testInfo.outputPath(`collaboration-${width}.png`), fullPage: true });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    }
  } finally {
    try {
      if (connection) {
        const work = `/director/v1/collaboration/${connection}/work`;
        const current = await post(work, { operation: 'background_status' });
        await post(work, { operation: 'background_configure', expected: current.policy, policy: null });
      }
      await request.delete(`/api/databases/${slug}`);
    } finally {
      remote.closeAllConnections();
      await new Promise<void>((resolve, reject) => remote.close(error => error ? reject(error) : resolve()));
      database.close();
      if (path.dirname(dir) === run && path.basename(dir).startsWith('tonight-')) fs.rmSync(dir, { recursive: true, force: true });
    }
  }
});
