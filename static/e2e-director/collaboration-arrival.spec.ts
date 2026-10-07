import { expect, test, type APIRequestContext } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema, fitsLight } from '../e2e/fixtures/sync';

async function api(request: APIRequestContext, method: string, url: string, data?: unknown) {
  const response = await request.fetch(url, { method, data });
  expect(response.ok(), `${method} ${url}: ${await response.text()}`).toBeTruthy();
  return (await response.json()).data;
}

test('arriving files offer only accepted images, review measured data, and replay a queued submission', async ({ page, request }, testInfo) => {
  test.setTimeout(90_000);
  const run = process.env.PSF_GUARD_DIRECTOR_E2E_TMP!;
  const root = mkdtempSync(path.join(run, 'contribution-arrival-'));
  const empty = path.join(root, 'empty');
  const incoming = path.join(root, 'incoming');
  mkdirSync(empty); mkdirSync(incoming);
  const slug = `arrival-${randomUUID().slice(0, 8)}`;
  const db = new Database(path.join(root, 'catalog.sqlite'));
  applyRealSchema(db);
  db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'profile','Arrival test','',1,1,0,0,?)").run(randomUUID());
  db.prepare("INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES(1,'Arrival target',1,0.509933,39.769,0,1,?)").run(randomUUID());
  db.prepare("INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,'profile','Ha 300','Ha',100,30,1,0,?)").run(randomUUID());
  db.prepare("INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,'profile',300,20,0,0,1,1,1,?)").run(randomUUID());

  const wire = JSON.parse(readFileSync(path.resolve('../crates/director-interop/tests/fixtures/starfront-tonight.json'), 'utf8'));
  delete wire.task;
  wire.tasks[0].share = [0];
  wire.tasks[0].visit = { seconds: 600, frames: { H: 2 }, filter: 'H', moon: 0.036 };
  wire.requirementsByProject['000000000002'].minFramesPerVisit = 2;
  wire.requirementsByProject['000000000002'].minMoonSeparation = null;
  wire.requirements = wire.requirementsByProject['000000000002'];
  const received: { contributions: Record<string, unknown>[] }[] = [];
  const unauthorized: string[] = [];
  let failDelivery = true;
  const remote = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(Buffer.from(chunk));
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {};
    res.setHeader('content-type', 'application/json');
    if (req.url?.startsWith('/api/v1/agent/') && req.headers.authorization !== 'Bearer local-test-agent-token') unauthorized.push(req.url);
    if (req.url === '/api/v1/health') res.end(JSON.stringify({ ok: true, protocol: 1, version: 'local test', time: Date.now() / 1000, features: ['pairing'] }));
    else if (req.url === '/api/v1/pair') res.end(JSON.stringify({ agent: { id: '000000000001' }, token: 'local-test-agent-token' }));
    else if (req.url === '/api/v1/agent/hello') res.end(JSON.stringify({ agent: '000000000001', protocol: 1, serverTime: Date.now() / 1000 }));
    else if (req.url?.startsWith('/api/v1/agent/task')) res.end(JSON.stringify(wire));
    else if (req.url === '/api/v1/agent/report') {
      received.push(body);
      if (failDelivery) { res.statusCode = 503; res.end(JSON.stringify({ detail: 'Offline test' })); }
      else res.end(JSON.stringify({ recorded: body.contributions.map(() => ({ id: '000000000010', accepted: true, duplicate: false, verdict: { accepted: true, reasons: [], unverified: [] } })) }));
    } else { res.statusCode = 404; res.end('{}'); }
  });
  await new Promise<void>(resolve => remote.listen(0, '127.0.0.1', resolve));
  const address = remote.address();
  if (!address || typeof address === 'string') throw new Error('Local receiver did not listen');
  const remoteUrl = `http://127.0.0.1:${address.port}/`;
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await api(request, 'POST', '/api/databases', { name: 'Arrival rig', slug, db_path: path.join(root, 'catalog.sqlite'), image_dirs: [empty, incoming] });
    await api(request, 'GET', '/api/director/v1/plans');
    const profile = await api(request, 'GET', `/api/director/v1/catalogs/${slug}/rig/profile`);
    const rig = profile.rig.id;
    await api(request, 'PUT', `/api/director/v1/catalogs/${slug}/rig/profile`, {
      expected_revision: profile.profile.revision,
      optics: { source: { kind: 'manual' }, value: { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 530, aperture_mm: null, rotation: { mode: 'rotator' } } },
      site: null, horizon: null, sky_quality: null,
      limits: { source: { kind: 'manual' }, value: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 88, meridian_exclusion: { before_ms: 600000, after_ms: 0 } } },
    });
    const connection = randomUUID();
    const route = `/api/director/v1/collaboration/${connection}`;
    await api(request, 'POST', `/api/director/v1/rigs/${rig}/collaboration`, { id: connection, server_url: remoteUrl, name: 'LOCAL TEST ONLY', allow_loopback_http: true });
    await api(request, 'POST', `${route}/discover`, {});
    await api(request, 'POST', `${route}/pair`, { code: 'LOCAL-TEST' });
    const work = (data: unknown) => api(request, 'POST', `${route}/work`, data);
    await work({ operation: 'configure', settings: { binning: 1, colour: false, hours_per_night: 6, share_status: false, filters: { Ha: { exposure_seconds: 300, bandpass_nm: 7 } } } });
    const night = { night: '2026-10-05', moon: 0.12, moon_up: 0.3 };
    const preview = await work({ operation: 'preview', task: '000000000004', night });
    await work({ operation: 'apply', task: '000000000004', night, review_digest: preview.preview.review_digest });

    const acquired = Math.floor(Date.parse('2026-10-06T04:00:00Z') / 1000);
    const guids = [randomUUID(), randomUUID(), randomUUID(), randomUUID()];
    const addRow = (id: number, grade: number) => db.prepare("INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(?,1,1,?,'Ha',?,?,'profile',1,?)")
      .run(id, acquired + id * 300, grade, JSON.stringify({ FileName: `Q:\\NINA\\arrival-${id}.fits`, HFR: id === 1 ? 0.5 : 1 }), guids[id - 1]);
    const save = (id: number) => {
      const file = path.join(incoming, `arrival-${id}.fits`);
      writeFileSync(file, fitsLight('Arrival target', '2026-10-06T04:00:00Z', ['FOCALLEN=                530.0']));
      const stat = statSync(file, { bigint: true });
      const canonical = realpathSync.native(file);
      const source = process.platform === 'win32' ? `\\\\?\\${canonical}` : canonical;
      const cache = path.join(run, 'cache', slug, 'astrometry');
      mkdirSync(cache, { recursive: true });
      // A persisted test-only pixel solution. This test exercises evidence
      // freshness and transport, not the astrometry solver's numeric accuracy.
      const analysis = {
        image_id: id, status: 'solved', mode: 'hinted', computed_at: acquired,
        solution: { center_ra_deg: 7.64899477, center_dec_deg: 39.769, pixel_scale_arcsec_per_pixel: 3.6, matched_stars: 30, rms_arcsec: 0.2, image_width: 10, image_height: 10,
          wcs: { crval: [7.64899477, 39.769], crpix: [5, 5], cd: [[-0.001, 0], [0, 0.001]], ctype: ['RA---TAN', 'DEC--TAN'], cunit: ['deg', 'deg'], radesys: 'ICRS', equinox: 2000 }, footprint: [], objects: [] },
        source_fingerprint: { canonical_path: source, size_bytes: Number(stat.size), modified_unix_seconds: Number(stat.mtimeNs / 1000000000n), modified_subsec_nanos: Number(stat.mtimeNs % 1000000000n) },
        solve_attempt: { outcome: 'solved', modes_attempted: ['hinted'], detected_stars: 30, duration_ms: 5, image_quality_evidence: true, cacheable: true },
      };
      writeFileSync(path.join(cache, `${id}.json.tmp`), JSON.stringify(analysis));
      renameSync(path.join(cache, `${id}.json.tmp`), path.join(cache, `${id}.json`));
    };
    addRow(1, 1); addRow(2, 0); addRow(3, 2);
    save(2); save(3);

    await page.clock.install();
    await page.goto('/#/');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.locator('.tauri-settings');
    await settings.getByRole('tab', { name: 'Rigs' }).click();
    await settings.getByRole('button', { name: 'Setup Arrival rig' }).click();
    const collaboration = settings.getByRole('region', { name: 'Collaboration', exact: true });
    await collaboration.getByRole('button', { name: 'Contribution reports' }).click();
    await collaboration.getByLabel('Imported visit').selectOption({ label: 'M31 halo in narrowband (2026-10-05)' });
    await collaboration.getByLabel('Observing night', { exact: true }).fill('2026-10-06');
    await collaboration.getByLabel('Rig database').selectOption(slug);
    await collaboration.getByLabel('Remote panel').selectOption('0');
    await expect(collaboration).toContainText('0 accepted images');
    save(1);
    await page.clock.fastForward(30_100);
    await expect(collaboration.getByLabel('Select arrival-1.fits')).toBeVisible();
    await expect(collaboration.getByLabel('Select arrival-2.fits')).toHaveCount(0);
    await expect(collaboration.getByLabel('Select arrival-3.fits')).toHaveCount(0);
    await collaboration.getByLabel('Select arrival-1.fits').check();
    addRow(4, 1); save(4);
    await page.clock.fastForward(30_100);
    await expect(collaboration).toContainText('2 accepted images');
    await expect(collaboration.getByLabel('Select arrival-4.fits')).not.toBeChecked();
    await collaboration.getByRole('button', { name: 'Select shown' }).click();
    await collaboration.getByRole('button', { name: 'Review 2 images' }).click();
    const reviewed = collaboration.getByRole('region', { name: 'Review contribution' });
    await expect(reviewed).toContainText('2 frames; 600 seconds; H; uncalibrated');
    await expect(reviewed).toContainText('2026-10-06');
    await expect(reviewed).toContainText('300.00 s');
    await expect(reviewed).toContainText('530.00 mm');
    await expect(reviewed).toContainText('2.70 arcsec');
    await expect(reviewed).toContainText('Guiding RMSUnknown');
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      await reviewed.scrollIntoViewIfNeeded();
      expect(await collaboration.evaluate(e => e.scrollWidth <= e.clientWidth)).toBeTruthy();
      await page.screenshot({ path: testInfo.outputPath(`accepted-contribution-${width}.png`) });
    }
    expect(received).toHaveLength(0);
    await collaboration.getByRole('button', { name: 'Queue finalized contribution' }).click();
    await expect(collaboration).toContainText('Contribution queued for check-in');
    expect(received).toHaveLength(0);
    await collaboration.getByRole('button', { name: 'Check in', exact: true }).click();
    await expect(collaboration.getByRole('alert')).toContainText('queued reports retained');
    expect(received).toHaveLength(1);
    failDelivery = false;
    await collaboration.getByRole('button', { name: 'Check in', exact: true }).click();
    await expect(collaboration).toContainText('1 reports delivered; 1 accepted; 0 rejected');
    expect(received).toHaveLength(2);
    expect(received[1]).toEqual(received[0]);
    expect(received[1].contributions[0]).toMatchObject({ project: '000000000002', task: '000000000004', night: '2026-10-06', panel: '0', filterName: 'H', frames: 2, seconds: 600, exposure: 300, focalLength: 530, hfr: 2.7, guideRms: null, calibrated: false, colour: false });
    await collaboration.getByRole('button', { name: 'Check in', exact: true }).click();
    await expect(collaboration).toContainText('0 reports delivered; 0 accepted; 0 rejected');
    expect(received).toHaveLength(2);
    expect(unauthorized).toEqual([]);
    expect(errors).toEqual([]);
  } finally {
    db.close();
    await request.delete(`/api/databases/${slug}`);
    remote.closeAllConnections();
    await new Promise<void>((resolve, reject) => remote.close(error => error ? reject(error) : resolve()));
  }
});
