import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fitsLight } from '../e2e/fixtures/sync';

test('Planning opens an image import and explains why a catalog copy is excluded', async ({ page, request }, testInfo) => {
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'planning-entry-'));
  const original = `a-images-${randomUUID().slice(0, 8)}`;
  const copy = `z-review-${randomUUID().slice(0, 8)}`;
  const originalPath = path.join(root, 'images.sqlite');
  const copyPath = path.join(root, 'review.sqlite');
  writeFileSync(path.join(root, 'm31.fits'), fitsLight('Imported M31', '2026-10-01T02:00:00', [
    'RA      =                10.68', 'DEC     =                41.27',
  ]));
  try {
    const created = await request.post('/api/databases/create', { data: {
      name: 'Image import', slug: original, db_path: originalPath, image_dirs: [root], backfill: false,
    } });
    expect(created.ok(), await created.text()).toBeTruthy();
    await expect.poll(async () => (await (await request.get(`/api/db/${original}/import`)).json()).data.progress.stage).toBe('complete');
    const projects = (await (await request.get(`/api/db/${original}/projects`)).json()).data;
    expect(projects).toHaveLength(1);
    const projectId = projects[0].id;
    expect(projects[0].guid).toBeTruthy();

    // A normal image-only import has a plan and can enter its workspace.
    await page.goto(`/#/grid?db=${original}&project=${projectId}`);
    const planning = page.getByRole('button', { name: 'Planning', exact: true });
    await expect(planning).toBeEnabled();
    await planning.click();
    await expect(page.getByRole('region', { name: 'Plan summary' })).toContainText('Imported M31');

    // SQLite backup preserves the catalog identity, exactly like a copied
    // file. Planning must keep excluding it rather than treating it as a rig.
    const source = new Database(originalPath, { readonly: true });
    try { await source.backup(copyPath); } finally { source.close(); }
    const added = await request.post('/api/databases', { data: {
      name: 'Calibration choices copy', slug: copy, db_path: copyPath, image_dirs: [root],
    } });
    expect(added.ok(), await added.text()).toBeTruthy();
    await page.goto(`/#/grid?db=${copy}&project=${projectId}`);
    await expect(planning).toBeEnabled();
    await planning.click();
    await expect(page).toHaveURL(new RegExp(`plan=${copy}%3A${projectId}`));
    const context = page.getByRole('region', { name: 'Project acquisition planning' });
    await expect(context).toContainText('carries the same catalog identity as Image import');
    await expect(context).toContainText('it is left out of planning');
    await expect(context.getByRole('button', { name: 'Retry planning' })).toBeEnabled();
    await expect(context.getByText('Fill in GUIDs')).toHaveCount(0);
    await expect(planning).toHaveAttribute('aria-current', 'page');
    await page.screenshot({ path: testInfo.outputPath('copied-catalog-planning.png'), fullPage: true });
    const db = new Database(copyPath, { readonly: true });
    try {
      expect(db.prepare('SELECT guid FROM project WHERE Id=?').get(projectId)).toEqual({ guid: projects[0].guid });
      expect(db.prepare('SELECT COUNT(*) AS count FROM psf_guard_catalog_identity').get()).toEqual({ count: 1 });
    } finally { db.close(); }
  } finally {
    await request.delete(`/api/databases/${copy}`);
    await request.delete(`/api/databases/${original}`);
  }
});
