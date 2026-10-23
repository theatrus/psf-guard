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

function count(databasePath: string, sql: string): number {
  const database = new Database(databasePath, { readonly: true });
  try {
    return (database.prepare(sql).get() as { n: number }).n;
  } finally {
    database.close();
  }
}

test('Delete removes an archived project after its name is typed, until it is allowed again', async ({ page, request }) => {
  const databasePath = catalogWithClosedBeta();
  await register(request, databasePath);

  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Open Project Alpha image grid' })).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: /Archived projects.*1/ }).click();
  await page.getByRole('button', { name: 'Delete Project Beta from Lifecycle rig' }).click();

  const dialog = page.getByRole('dialog', { name: 'Delete Project Beta' });
  await expect(dialog.getByText('1 target and 0 exposure plans')).toBeVisible();
  await expect(dialog.getByText(/^1 frame \(0 accepted, 0 rejected, 1 pending\)/)).toContainText('The image files stay on disk.');
  const confirm = dialog.getByRole('button', { name: 'Delete project' });
  await expect(confirm).toBeDisabled();
  await dialog.getByRole('textbox', { name: /Type Project Beta/ }).fill('Project Beta');
  await confirm.click();

  await expect(page.getByRole('status').filter({ hasText: 'Deleted Project Beta from Lifecycle rig, with 1 target and 1 frame.' })).toBeVisible();
  await expect(page.locator('.project-archive-item').filter({ hasText: 'Project Beta' })).toHaveCount(0);
  expect(count(databasePath, "SELECT COUNT(*) AS n FROM project WHERE name = 'Project Beta'")).toBe(0);
  expect(count(databasePath, 'SELECT COUNT(*) AS n FROM acquiredimage WHERE projectId = 2')).toBe(0);
  expect(count(databasePath, 'SELECT COUNT(*) AS n FROM acquiredimage WHERE projectId = 1')).toBeGreaterThan(0);
  expect(count(databasePath, 'SELECT COUNT(*) AS n FROM psf_guard_removed_project_frame')).toBe(1);

  const deleted = page.getByRole('region', { name: 'Deleted projects' });
  await deleted.getByRole('button', { name: /Deleted projects/ }).click();
  await deleted.getByRole('button', { name: 'Allow Project Beta back on Lifecycle rig' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Project Beta can come back to Lifecycle rig' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Deleted projects' })).toHaveCount(0);
  expect(count(databasePath, 'SELECT COUNT(*) AS n FROM psf_guard_removed_project_frame')).toBe(0);
});

test('Edit project opens the delete dialog, and Cancel deletes nothing', async ({ page, request }) => {
  const databasePath = catalogWithClosedBeta();
  await register(request, databasePath);

  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Open Project Alpha image grid' })).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: 'Edit project' }).first().click();
  await page.getByRole('button', { name: 'Delete project...' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete Project Alpha' });
  await expect(dialog.getByRole('textbox', { name: /Type Project Alpha/ })).toBeVisible();
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).toHaveCount(0);
  expect(count(databasePath, "SELECT COUNT(*) AS n FROM project WHERE name = 'Project Alpha'")).toBe(1);
});

test('a plan with nothing captured is deleted from the Library', async ({ page, request }) => {
  await register(request, catalogWithClosedBeta());
  const id = randomUUID();
  const created = await request.post('/api/director/v1/projects', { data: { id, name: 'Lifecycle plan' } });
  expect(created.ok(), await created.text()).toBeTruthy();

  await page.goto('/');
  const plans = page.getByRole('region', { name: 'Plans with nothing captured yet' });
  await expect(plans.getByRole('link', { name: 'Open Lifecycle plan' })).toBeVisible({ timeout: 15_000 });
  await plans.getByRole('button', { name: 'Delete Lifecycle plan' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete Lifecycle plan' });
  await expect(dialog.getByText('its Framing, Exposures and Priority settings, and its activation record')).toBeVisible();
  await dialog.getByRole('button', { name: 'Delete plan' }).click();

  await expect(plans.getByRole('status')).toHaveText('Deleted the plan Lifecycle plan.');
  await expect(plans.getByRole('link', { name: 'Open Lifecycle plan' })).toHaveCount(0);
  const gone = await request.get(`/api/director/v1/projects/${id}`);
  expect(gone.status()).toBe(404);
});
