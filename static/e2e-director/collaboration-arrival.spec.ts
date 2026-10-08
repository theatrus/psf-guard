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

for (const executor of ['Target Scheduler', 'Director']) {
test(`${executor}: arriving M31 files review measured data and replay a queued submission`, async ({ page, request }, testInfo) => {
  test.setTimeout(120_000);
  const run = process.env.PSF_GUARD_DIRECTOR_E2E_TMP!;
  const root = mkdtempSync(path.join(run, 'contribution-arrival-'));
  const empty = path.join(root, 'empty');
  const incoming = path.join(root, 'incoming');
  mkdirSync(empty); mkdirSync(incoming);
  const slug = `arrival-${randomUUID().slice(0, 8)}`;
  const db = new Database(path.join(root, 'catalog.sqlite'));
  applyRealSchema(db);
  db.prepare('INSERT INTO profilepreference(profileId,guid) VALUES(?,?)').run('profile', randomUUID());
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
  let taskReads = 0;
  const remote = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(Buffer.from(chunk));
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {};
    res.setHeader('content-type', 'application/json');
    if (req.url?.startsWith('/api/v1/agent/') && req.headers.authorization !== 'Bearer local-test-agent-token') unauthorized.push(req.url);
    if (req.url === '/api/v1/health') res.end(JSON.stringify({ ok: true, protocol: 1, version: 'local test', time: Date.now() / 1000, features: ['pairing'] }));
    else if (req.url === '/api/v1/pair') res.end(JSON.stringify({ agent: { id: '000000000001' }, token: 'local-test-agent-token' }));
    else if (req.url === '/api/v1/agent/hello') res.end(JSON.stringify({ agent: '000000000001', protocol: 1, serverTime: Date.now() / 1000 }));
    else if (req.url?.startsWith('/api/v1/agent/task')) { taskReads++; res.end(JSON.stringify(wire)); }
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
  let connectionRoute: string | undefined;
  try {
    await api(request, 'POST', '/api/databases', { name: 'Arrival rig', slug, db_path: path.join(root, 'catalog.sqlite'), image_dirs: [empty, incoming] });
    await api(request, 'GET', '/api/director/v1/plans');
    const profile = await api(request, 'GET', `/api/director/v1/catalogs/${slug}/rig/profile`);
    const rig = profile.rig.id;
    await api(request, 'PUT', `/api/director/v1/catalogs/${slug}/rig/profile`, {
      expected_revision: profile.profile.revision,
      optics: { source: { kind: 'manual' }, value: { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 530, aperture_mm: null, rotation: { mode: 'rotator' } } },
      site: { source: { kind: 'manual' }, value: { latitude_degrees: 35, longitude_degrees: -120, elevation_meters: 1000 } }, horizon: null, sky_quality: null,
      limits: { source: { kind: 'manual' }, value: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 88, meridian_exclusion: { before_ms: 600000, after_ms: 0 } } },
    });
    const connection = randomUUID();
    const route = `/api/director/v1/collaboration/${connection}`;
    connectionRoute = route;
    await api(request, 'POST', `/api/director/v1/rigs/${rig}/collaboration`, { id: connection, server_url: remoteUrl, name: 'LOCAL TEST ONLY', allow_loopback_http: true });
    await api(request, 'POST', `${route}/discover`, {});
    await api(request, 'POST', `${route}/pair`, { code: 'LOCAL-TEST' });
    const work = (data: unknown) => api(request, 'POST', `${route}/work`, data);
    await work({ operation: 'configure', settings: { binning: 1, colour: false, hours_per_night: 6, share_status: false, filters: { Ha: { exposure_seconds: 300, bandpass_nm: 7 } } } });
    const night = { night: '2026-10-05', moon: 0.12, moon_up: 0.3 };
    const preview = await work({ operation: 'preview', task: '000000000004', night });
    const imported = await work({ operation: 'apply', task: '000000000004', night, review_digest: preview.preview.review_digest });
    const project = imported.plan.project_id;
    const draft = (await api(request, 'GET', `/api/director/v1/projects/${project}/plan`)).plan;
    draft.contributions = [{ id: randomUUID(), objective_id: draft.objectives[0].id, rig_id: rig,
      template: { template_guid: null, template_id: null, name: 'M31 Ha', filter_name: 'Ha', gain: 100, offset: 30, bin: 1, readout_mode: 0, moon: null },
      exposure_seconds: 300, panel_ids: [], enabled: true }];
    await api(request, 'PUT', `/api/director/v1/projects/${project}/plan`, draft);
    const collaborationVisit = [{ import_id: imported.plan.import_id, source_digest: imported.plan.digest }];
    const activation = await api(request, 'POST', `/api/director/v1/projects/${project}/activation/preview`, { collaboration: collaborationVisit });
    await api(request, 'POST', `/api/director/v1/projects/${project}/activation/apply`, { preview_digest: activation.preview_digest, collaboration: collaborationVisit });
    const assigned = db.prepare('SELECT e.Id AS exposure_id,e.guid AS goal,t.Id AS target_id,t.projectId AS project_id,t.name FROM exposureplan e JOIN target t ON t.Id=e.targetId JOIN psf_guard_collaboration_plan c ON c.exposureplan_guid=e.guid WHERE c.import_id=?').get(imported.plan.import_id) as { exposure_id: number; goal: string; target_id: number; project_id: number; name: string };

    const acquired = Math.floor(Date.now() / 1000) + 1;
    const observingNight = new Date((acquired - 20 * 3600) * 1000).toISOString().slice(0, 10);
    const guids = [randomUUID(), randomUUID(), randomUUID(), randomUUID()];
    const captures = guids.map(() => randomUUID());
    const exposure = executor === 'Director' ? 300.007 : 300;
    const addRow = (id: number, grade: number) => db.prepare("INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(?,?,?,?,'Ha',?,?,'profile',?,?)")
      .run(id, assigned.project_id, assigned.target_id, acquired + id * 300, grade, JSON.stringify({ FileName: `Q:\\NINA\\arrival-${id}.fits`, HFR: id === 1 ? 0.5 : 1 }), assigned.exposure_id, guids[id - 1]);
    const save = (id: number) => {
      const file = path.join(incoming, `arrival-${id}.fits`);
      const headers = ['FOCALLEN=                530.0', 'SITELAT =                 35.0', 'SITELONG=               -120.0', 'SITEELEV=               1000.0', 'PGBAND  =                  7.0'];
      if (executor === 'Director') headers.push(`PGCAPID = '${captures[id - 1]}'`, 'PGGRMS  =                  0.5', `PGHFR   =                  ${id === 1 ? '0.5' : '1.0'}`);
      writeFileSync(file, fitsLight(assigned.name, new Date((acquired + id * 300) * 1000).toISOString(), headers, exposure));
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
    await settings.getByRole('tab', { name: 'Collaboration', exact: true }).click();
    const collaboration = settings.getByRole('region', { name: 'Collaboration', exact: true });
    await collaboration.getByRole('tab', { name: 'Reports' }).click();
    await collaboration.getByLabel('Imported visit').selectOption({ label: 'M31 halo in narrowband (2026-10-05)' });
    await collaboration.getByLabel('Observing night', { exact: true }).fill(observingNight);
    await expect(collaboration.getByRole('combobox', { name: 'Rig database', exact: true })).toHaveCount(0);
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
    if (executor === 'Director') {
      await expect(collaboration.getByRole('alert')).toContainText('Director capture check-in is missing');
      const identity = await api(request, 'GET', '/api/director/v1/status');
      const mapping = await api(request, 'GET', `/api/director/v1/catalogs/${slug}/mappings`);
      const ledger = randomUUID();
      const events = [1, 4].flatMap((id, index) => ['reserved', 'saved'].map((state, step) => ({
        schema_version: 1, ledger_id: ledger, sequence: index * 2 + step + 1,
        rig_id: rig, attempt: { capture_id: captures[id - 1], goal_id: assigned.goal,
          reserved_at_ms: (acquired + id * 300) * 1000,
          evidence: state === 'reserved' ? { state } : { state, image_id: captures[id - 1], elapsed_ms: 301000 } },
      })));
      const checkin = { coordinator_instance_id: identity.instance_id, catalog_id: mapping.catalog_identity.id, ledger_id: ledger, events };
      const checked = await api(request, 'POST', `/api/director/v1/rigs/${rig}/checkin`, checkin);
      expect(checked.applied).toBe(4);
      const replay = await api(request, 'POST', `/api/director/v1/rigs/${rig}/checkin`, checkin);
      expect(replay.duplicates).toBe(4);
      await collaboration.getByRole('button', { name: 'Review 2 images' }).click();
      // Capture receipts never replace the catalog's stable sync identities.
      expect(db.prepare('SELECT guid FROM acquiredimage ORDER BY Id').all()).toEqual(guids.map(guid => ({ guid })));
    }
    const reviewed = collaboration.getByRole('region', { name: 'Review contribution' });
    await expect(reviewed).toContainText(`2 frames; ${exposure * 2} seconds; H; uncalibrated`);
    await expect(reviewed).toContainText(observingNight);
    await expect(reviewed).toContainText(`${exposure.toFixed(2)} s`);
    await expect(reviewed).toContainText('530.00 mm');
    await expect(reviewed).toContainText('2.70 arcsec');
    await expect(reviewed).toContainText(executor === 'Director' ? '0.50 arcsec' : 'Guiding RMSUnknown');
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
    // Grade a saved frame later. Automatic reporting extends the same capture
    // night's aggregate and replays the exact immutable body after an outage.
    await collaboration.getByRole('button', { name: 'Check in', exact: true }).click();
    await expect(collaboration).toContainText('0 reports delivered; 0 accepted; 0 rejected');
    expect(received).toHaveLength(2);
    db.prepare('UPDATE acquiredimage SET gradingStatus=1 WHERE Id=2').run();
    const mapping = await api(request, 'GET', `/api/director/v1/catalogs/${slug}/mappings`);
    if (executor === 'Director') {
      const identity = await api(request, 'GET', '/api/director/v1/status');
      const ledger = randomUUID();
      const events = ['reserved', 'saved'].map((state, step) => ({
        schema_version: 1, ledger_id: ledger, sequence: step + 1, rig_id: rig,
        attempt: { capture_id: captures[1], goal_id: assigned.goal, reserved_at_ms: (acquired + 600) * 1000,
          evidence: state === 'reserved' ? { state } : { state, image_id: captures[1], elapsed_ms: 301000 } },
      }));
      await api(request, 'POST', `/api/director/v1/rigs/${rig}/checkin`, { coordinator_instance_id: identity.instance_id, catalog_id: mapping.catalog_identity.id, ledger_id: ledger, events });
    }
    failDelivery = true;
    const readsBeforeReporting = taskReads;
    const policy = { enabled: false, catalog_id: mapping.catalog_identity.id, project_ids: [], interval_minutes: 15, activate: false, automatic_reports: true };
    await work({ operation: 'background_configure', expected: null, policy });
    await page.close();
    await expect.poll(() => received.length, { timeout: 40_000 }).toBeGreaterThan(2);
    expect(received.at(-1)?.contributions[0]).toMatchObject({ night: observingNight, frames: 3 });
    expect(received.at(-1)?.contributions[0].seconds).toBeCloseTo(exposure * 3, 6);
    const attempted = received.at(-1);
    failDelivery = false;
    const resumed = await work({ operation: 'background_run' });
    expect(resumed.status.reports.delivered).toBe(1);
    expect(received.at(-1)).toEqual(attempted);
    const deliveredCount = received.length;
    const repeated = await work({ operation: 'background_run' });
    expect(repeated.status.reports).toMatchObject({ queued: 0, delivered: 0, held: 0 });
    expect(received).toHaveLength(deliveredCount);
    expect(taskReads).toBe(readsBeforeReporting);
    await work({ operation: 'background_configure', expected: policy, policy: null });
    expect(received[1]).toEqual(received[0]);
    expect(received[1].contributions[0]).toMatchObject({ project: '000000000002', task: '000000000004', night: observingNight, panel: '0', filterName: 'H', frames: 2, seconds: exposure * 2, exposure, focalLength: 530, hfr: 2.7, guideRms: executor === 'Director' ? 0.5 : null, calibrated: false, colour: false, bandpass: 7 });
    expect(received[1].contributions[0].moonIllumination).toBeGreaterThanOrEqual(0);
    expect(received[1].contributions[0].moonSeparation).toBeGreaterThanOrEqual(0);
    await testInfo.attach('local-M31-contribution.json', { body: JSON.stringify(received[1], null, 2), contentType: 'application/json' });
    expect(unauthorized).toEqual([]);
    expect(errors).toEqual([]);
  } finally {
    if (connectionRoute) {
      const current = await api(request, 'POST', `${connectionRoute}/work`, { operation: 'background_status' });
      if (current.policy) await api(request, 'POST', `${connectionRoute}/work`, { operation: 'background_configure', expected: current.policy, policy: null });
    }
    db.close();
    await request.delete(`/api/databases/${slug}`);
    remote.closeAllConnections();
    await new Promise<void>((resolve, reject) => remote.close(error => error ? reject(error) : resolve()));
  }
});
}
