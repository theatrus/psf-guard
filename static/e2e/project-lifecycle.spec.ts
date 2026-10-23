import { expect, test, type APIRequestContext } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'crypto';
import * as path from 'path';
import { applyRealSchema } from './fixtures/sync';
import { fixtureDbPath, fixtureImageDir, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

// Reopening a closed project from the Library's archive, on a copy of the
// fixture catalog so the state write stays out of other specs.

/** A catalog on the full Target Scheduler schema, which the state write
 *  needs, with the suite's real frames: Project Alpha Active and Project
 *  Beta Closed. */
function catalogWithClosedBeta(): string {
  const source = new Database(fixtureDbPath(), { readonly: true });
  const frames = source.prepare('SELECT Id, acquireddate, metadata FROM acquiredimage ORDER BY Id').all() as Array<{
    Id: number; acquireddate: number; metadata: string;
  }>;
  source.close();
  const databasePath = path.join(tmpBase(), `project-lifecycle-${randomUUID()}.sqlite`);
  const database = new Database(databasePath);
  try {
    applyRealSchema(database);
    database.exec(`
      INSERT INTO project (Id,profileId,name,state,priority,isMosaic,flatsHandling,guid)
        VALUES (1,'default','Project Alpha',1,1,0,0,'lifecycle-alpha'),
               (2,'default','Project Beta',3,1,0,0,'lifecycle-beta');
      INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid)
        VALUES (1,'Alpha target',1,8.6738,19.66015,0,1,'lifecycle-alpha-target'),
               (2,'Beta target',1,8.6738,19.66015,0,2,'lifecycle-beta-target');
    `);
    const insert = database.prepare(`
      INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,guid)
      VALUES (?, ?, ?, ?, 'B', 0, ?, 'default', ?)
    `);
    for (const frame of frames) {
      const project = frame.Id === frames[frames.length - 1].Id ? 2 : 1;
      insert.run(frame.Id, project, project, frame.acquireddate, frame.metadata, `lifecycle-image-${frame.Id}`);
    }
  } finally {
    database.close();
  }
  return databasePath;
}

function projectState(databasePath: string, id: number): number {
  const database = new Database(databasePath, { readonly: true });
  try {
    return (database.prepare('SELECT state FROM project WHERE Id = ?').get(id) as { state: number }).state;
  } finally {
    database.close();
  }
}

async function register(request: APIRequestContext, databasePath: string): Promise<string> {
  await resetDatabases(request);
  const registered = await request.post('/api/databases', {
    data: { name: 'Lifecycle rig', slug: `lifecycle-${randomUUID().slice(0, 8)}`, db_path: databasePath, image_dirs: [fixtureImageDir()] },
  });
  expect(registered.ok(), await registered.text()).toBeTruthy();
  const dbId = (await registered.json()).data.id as string;
  await waitForCacheReady(request, dbId);
  return dbId;
}

test('Reopen moves an archived project back to the list as Inactive', async ({ page, request }) => {
  const databasePath = catalogWithClosedBeta();
  await register(request, databasePath);

  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Open Project Alpha image grid' })).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: /Archived projects.*1/ }).click();
  await page.getByRole('button', { name: 'Reopen Project Beta on Lifecycle rig' }).click();

  await expect(page.getByRole('status').filter({ hasText: 'Project Beta is now Inactive on Lifecycle rig.' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open Project Beta image grid' })).toBeVisible();
  await expect(page.locator('.project-archive')).toHaveCount(0);
  expect(projectState(databasePath, 2)).toBe(2);
});
