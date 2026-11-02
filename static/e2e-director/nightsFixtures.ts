// Rigs, their Target Scheduler databases and screenshots for the Nights
// specs.
import { expect, type APIRequestContext } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

/** A rig database with Target Scheduler projects of one N.I.N.A. profile. */
export function rigDatabase(projects: Array<{ name: string; priority: number; target: string; ra_hours: number; dec: number; filters: Array<[string, number]>; moon?: boolean; switchEvery?: number; customHorizon?: boolean }>): string {
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'nights-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  let template = 0;
  let plan = 0;
  projects.forEach((project, index) => {
    const id = index + 1;
    db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,minimumtime,minimumaltitude,usecustomhorizon,horizonoffset,meridianwindow,filterswitchfrequency,ditherevery,enablegrader,isMosaic,flatsHandling,guid) VALUES(?,'profile-a',?,'',1,?,30,25,?,0,0,?,0,0,0,0,?)")
      .run(id, project.name, project.priority, project.customHorizon ? 1 : 0, project.switchEvery ?? 0, randomUUID());
    db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,rotation,roi,projectid,guid) VALUES(?,?,1,?,?,2,0,100,?,?)')
      .run(id, project.target, project.ra_hours, project.dec, id, randomUUID());
    for (const [filter, desired] of project.filters) {
      template += 1;
      plan += 1;
      db.prepare("INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,twilightlevel,moonavoidanceenabled,moonavoidanceseparation,moonavoidancewidth,defaultexposure,guid) VALUES(?,'profile-a',?,?,100,30,1,0,0,?,60,7,300,?)")
        .run(template, `${project.name} ${filter}`, filter, project.moon ? 1 : 0, randomUUID());
      db.prepare("INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(?,'profile-a',300,?,0,0,?,?,1,?)")
        .run(plan, desired, id, template, randomUUID());
    }
  });
  db.close();
  return dbPath;
}

/** Register a rig database, make it a rig, and give the rig its own
 *  location with a 25° minimum. Returns the rig's id. */
export async function addRig(request: APIRequestContext, name: string, slug: string, dbPath: string, site: { latitude_degrees: number; longitude_degrees: number }) {
  const json = async (response: Awaited<ReturnType<APIRequestContext['post']>>) => {
    expect(response.ok(), await response.text()).toBeTruthy();
    return (await response.json()).data;
  };
  await json(await request.post('/api/databases', { data: { name, slug, db_path: dbPath, image_dirs: [path.dirname(dbPath)] } }));
  const catalog = randomUUID();
  const reviewed = await json(await request.post(`/api/director/v1/catalogs/${slug}/rig/preview`, { data: { catalog_id: catalog } }));
  await json(await request.post(`/api/director/v1/catalogs/${slug}/rig/apply`, { data: { plan: { catalog_id: catalog }, preview_digest: reviewed.preview_digest } }));
  const profile = (await json(await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`))).profile;
  await json(await request.put(`/api/director/v1/catalogs/${slug}/rig/profile`, { data: {
    expected_revision: profile.revision,
    optics: null,
    site: { value: { ...site, elevation_meters: 400 }, source: { kind: 'manual' } },
    horizon: null, sky_quality: null,
    limits: { value: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, meridian_exclusion: { before_ms: 0, after_ms: 0 } }, source: { kind: 'manual' } },
    peer_id: null,
  } }));
  return (await json(await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`))).rig.id as string;
}

/** A screenshot in the test's output, and with PSF_GUARD_CAPTURE_DOCS=1
 *  also in docs/images for the documentation. */
export async function shot(target: { screenshot: (options: { path: string; fullPage?: boolean; animations?: 'disabled' }) => Promise<unknown> }, name: string, outputPath: (name: string) => string, fullPage = true) {
  await target.screenshot({ path: outputPath(`${name}.png`), ...(fullPage ? { fullPage } : {}), animations: 'disabled' });
  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    await target.screenshot({ path: path.resolve(process.cwd(), '..', 'docs', 'images', `${name}.png`), ...(fullPage ? { fullPage } : {}), animations: 'disabled' });
  }
}
