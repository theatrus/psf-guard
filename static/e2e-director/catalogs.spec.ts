import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

test('every database project is a plan, shared GUIDs make one plan across rigs, and the workspace opens from the Library', async ({ page, request }, testInfo) => {
  test.setTimeout(60_000);
  const slugs: string[] = [];
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const shared = randomUUID();
  try {
    for (const [index, name] of ['C925 data', 'Redcat data'].entries()) {
      const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'catalog-'));
      const dbPath = path.join(root, 'catalog.sqlite');
      const db = new Database(dbPath);
      applyRealSchema(db);
      const insert = db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(?,?,?,'',1,1,0,0,?)");
      // The same GUID in both databases, the way Sync copies a project.
      insert.run(1, 'current-profile', 'Andromeda exposures', shared);
      if (index === 0) {
        insert.run(2, 'previous-profile', 'Andromeda older setup', randomUUID());
        insert.run(3, 'current-profile', 'Legacy project without GUID', null);
      }
      db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES(1,?,1,0.712313,41.2687,2,1,?)').run('Andromeda target', randomUUID());
      db.prepare('INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,?,?,?,100,30,1,0,?)').run('current-profile', 'Ha', 'Ha', randomUUID());
      db.prepare('INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,?,?,40,1,1,1,1,1,?)').run('current-profile', index === 0 ? 300 : 60, randomUUID());
      db.prepare('INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(1,1,1,1750000000,?,1,?,?,1,?)').run('Ha', JSON.stringify({ FileName: 'not-copied-yet.fits' }), 'current-profile', randomUUID());
      db.close();
      const slug = `director-${randomUUID().slice(0, 8)}`;
      const added = await request.post('/api/databases', { data: { name, slug, db_path: dbPath, image_dirs: [root] } });
      expect(added.ok(), await added.text()).toBeTruthy();
      slugs.push(slug);
    }

    // Opening the Library adopts both databases: the shared GUID is one plan
    // shot by two rigs, an outer pill with a card per rig.
    await page.goto('/#/?db=parked-catalog');
    const andromeda = page.getByTestId('library-family').filter({ hasText: 'Andromeda exposures' });
    await expect(andromeda).toHaveCount(1);
    await expect(andromeda.getByText('2 rigs')).toBeVisible();
    await expect(andromeda.getByText('2 / 80 · 3%')).toBeVisible();
    // Its Target Scheduler targets and exposure plans were imported as a draft plan.
    await expect(andromeda.getByText('Planned', { exact: true })).toBeVisible();
    await expect(andromeda.locator('[data-project-key]')).toHaveCount(2);
    // A plan with nothing captured yet has no Library row; it waits in the plans section.
    await expect(page.getByRole('region', { name: 'Plans with nothing captured yet' }).getByTestId('plan-row').filter({ hasText: 'Andromeda older setup' })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'New rig' })).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath('library-plans-desktop.png'), fullPage: true });
    const mappings = await Promise.all(slugs.map(async slug => (await (await request.get(`/api/director/v1/catalogs/${slug}/mappings`)).json()).data));
    expect(mappings[0].items).toHaveLength(2);
    expect(mappings[1].items).toHaveLength(1);
    expect(mappings[0].items.find((item: { source_project_guid: string }) => item.source_project_guid === shared).project_id).toBe(mappings[1].items[0].project_id);
    expect(mappings[0].rig.id).not.toBe(mappings[1].rig.id);

    // Rig setup lives under Settings › Rigs; Setup expands the rig profile in place.
    await page.getByRole('button', { name: 'Settings' }).click();
    const settings = page.locator('.tauri-settings');
    await settings.getByRole('tab', { name: 'Rigs' }).click();
    await settings.getByRole('button', { name: 'Setup C925 data' }).click();
    await expect(settings.getByRole('region', { name: 'Rig profile' })).toBeVisible();
    await expect(settings.getByRole('button', { name: 'Save rig profile' })).toBeVisible();
    const activeRig = settings.locator('.director-rig.is-open');
    await expect(activeRig).toHaveCount(1);
    await expect(activeRig.getByRole('heading', { name: 'C925 data', exact: true })).toBeVisible();
    const setupButton = activeRig.getByRole('button', { name: 'Setup C925 data' });
    expect(await setupButton.getAttribute('aria-controls')).toBe(await activeRig.locator(':scope > div[id]').getAttribute('id'));
    // A section rail distinguishes rig setup from Settings and collaboration tabs.
    for (const width of [1440, 900, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      const nav = activeRig.getByRole('tablist', { name: 'Rig setup sections' });
      await expect(nav).toHaveAttribute('aria-orientation', width >= 800 ? 'vertical' : 'horizontal');
      const railBox = await nav.boundingBox();
      const contentBox = await activeRig.locator('.rig-setup-content').boundingBox();
      expect(railBox).not.toBeNull(); expect(contentBox).not.toBeNull();
      if (width >= 800) expect(contentBox!.x).toBeGreaterThanOrEqual(railBox!.x + railBox!.width - 1);
      else expect(contentBox!.y).toBeGreaterThanOrEqual(railBox!.y + railBox!.height - 1);
      expect(await activeRig.evaluate(e => e.scrollWidth <= e.clientWidth)).toBe(true);
      expect((await activeRig.getByRole('combobox', { name: 'Camera angle', exact: true }).boundingBox())!.width).toBeGreaterThanOrEqual(140);
      await settings.locator('.modal-body').evaluate(e => { e.scrollTop = 0; });
      await page.screenshot({ path: testInfo.outputPath(`rig-setup-hierarchy-${width}.png`) });
    }
    await page.setViewportSize({ width: 1440, height: 1000 });
    await settings.getByRole('button', { name: 'Setup Redcat data' }).click();
    await expect(settings.locator('.director-rig.is-open')).toHaveCount(1);
    await expect(settings.locator('.director-rig.is-open').getByRole('heading', { name: 'Redcat data', exact: true })).toBeVisible();
    await expect(settings.getByRole('button', { name: 'Setup C925 data' })).toHaveAttribute('aria-expanded', 'false');
    await settings.getByRole('button', { name: '×' }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);

    // The outer pill opens the workspace: one block per rig, each with its
    // database's project and its own editor.
    await andromeda.getByRole('button', { name: 'Open the Andromeda exposures plan' }).click();
    await expect(page.getByRole('heading', { name: 'Andromeda exposures' })).toBeVisible();
    // One tab at a time below the summary; each rig's readiness is in the summary.
    const tab = (name: string) => page.getByRole('tablist', { name: 'Plan sections' }).getByRole('tab', { name: new RegExp(`^${name}`) });
    await expect(page.getByRole('region', { name: 'Plan summary' })).toContainText('C925 data');
    // The Rigs tab lists each rig with its database's project, the
    // database the outer pill came from first, and that project's own
    // Target Scheduler settings and targets.
    await tab('Rigs').click();
    const rigs = page.getByRole('region', { name: 'Rigs' });
    await expect(rigs.getByRole('group', { name: 'C925 data' })).toContainText('project');
    await expect(rigs.getByRole('group', { name: 'Redcat data' })).toContainText('project');
    await expect(page.getByLabel('RA (decimal hours)').first()).toHaveValue('0.712313');
    await expect(page.getByLabel('Ha desired count').first()).toHaveValue('40');
    await tab('Exposures').click();
    await expect(page.getByRole('region', { name: 'Acquisition plan' })).toBeVisible();
    // The plan's own scheduling limit, on the Rigs tab, saved from the bar and shown per rig.
    await tab('Rigs').click();
    const limits = page.getByRole('region', { name: 'Scheduling limits' });
    await limits.getByRole('spinbutton', { name: 'Minimum altitude' }).fill('30');
    await expect(tab('Rigs')).toContainText('●');
    await page.getByRole('region', { name: 'Unsaved changes' }).getByRole('button', { name: 'Save changes' }).click();
    await expect(limits.getByRole('row', { name: /^Minimum altitude/ })).toContainText('30°from this plan');
    // Activation is no tab: with the plan not yet on the rigs, the bar at
    // the top asks for it, and the summary leaves that to the bar.
    await expect(page.getByRole('tab', { name: /^Activate/ })).toHaveCount(0);
    await expect(page.getByTestId('summary-activation')).toHaveCount(0);
    await page.getByRole('region', { name: 'Activation due' }).getByRole('button', { name: 'Activate…' }).click();
    const activation = page.getByRole('dialog', { name: 'Activate on the rigs' });
    await expect(activation.getByRole('region', { name: 'Activation' })).toBeVisible();
    await activation.getByRole('button', { name: /close/i }).click();
    await tab('Priority and defaults').click();
    const preferences = page.getByRole('region', { name: 'Project priority' });
    await preferences.getByRole('button', { name: 'Move Andromeda older setup up' }).click();
    // Every edit on the page saves from one bar, which names what changed.
    await expect(page.getByRole('region', { name: 'Unsaved changes' })).toContainText('Unsaved changes in Priority and defaults.');
    await expect(preferences.getByLabel('Priority scope')).toBeDisabled();
    await page.getByRole('region', { name: 'Unsaved changes' }).getByRole('button', { name: 'Save changes' }).click();
    await expect(preferences.getByText('Project priority saved.')).toBeVisible();
    await expect(preferences.getByText('Following global order')).toBeVisible();
    await page.reload();
    await expect(preferences.getByRole('listitem').first()).toContainText('Andromeda older setup');
    await preferences.getByLabel('Priority scope').selectOption('rig');
    await expect(preferences.getByLabel('Use inherited order')).toBeChecked();
    await expect(preferences.getByRole('button', { name: 'Move Andromeda exposures up' })).toBeDisabled();
    await preferences.getByLabel('Use inherited order').uncheck();
    await preferences.getByRole('button', { name: 'Move Andromeda exposures up' }).click();
    await page.getByRole('region', { name: 'Unsaved changes' }).getByRole('button', { name: 'Save changes' }).click();
    await expect(preferences.getByText('Following rig order')).toBeVisible();
    await preferences.getByLabel('Use inherited order').check();
    await page.getByRole('region', { name: 'Unsaved changes' }).getByRole('button', { name: 'Save changes' }).click();
    await expect(preferences.getByText('Following global order')).toBeVisible();
    await preferences.getByLabel('Priority scope').selectOption('global');
    await preferences.scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath('project-priority-desktop.png') });
    await page.setViewportSize({ width: 375, height: 812 });
    await preferences.scrollIntoViewIfNeeded();
    expect(await preferences.evaluate(element => element.scrollWidth <= element.clientWidth)).toBeTruthy();
    await page.screenshot({ path: testInfo.outputPath('project-priority-mobile.png') });
    await page.screenshot({ path: testInfo.outputPath('project-workspace-mobile.png'), fullPage: true });
    expect(await page.locator('.director-page').evaluate(element => element.scrollWidth <= element.clientWidth)).toBeTruthy();
    await page.setViewportSize({ width: 1440, height: 1000 });

    // The Library card's Plan button lands in the same workspace.
    await page.goto(`/#/?db=${slugs[0]}&project=1&dbfilter=${slugs[0]}`);
    const card = page.locator(`[data-project-key="${slugs[0]}:1"]`);
    await card.getByRole('button', { name: /^Open the .* plan$/ }).click();
    await expect(page.getByRole('heading', { name: 'Andromeda exposures' })).toBeVisible();
    // The database the card came from leads the Rigs tab.
    await tab('Rigs').click();
    await expect(page.getByLabel('RA (decimal hours)').first()).toHaveValue('0.712313');
    // The workspace's address is the Target Scheduler GUID both rigs share.
    expect(new URL(page.url().split('#')[1], 'http://test').searchParams.get('plan')).toBe(shared.toLowerCase());
    await page.getByRole('main').getByRole('link', { name: 'Library' }).click();
    await expect(page.getByRole('heading', { name: 'Projects' })).toBeVisible();
    expect(page.url()).toContain(`dbfilter=${slugs[0]}`);

    // The header picker shows the shared project once, opening to each rig.
    await page.goto(`/#/grid?db=${slugs[0]}&project=1`);
    const trigger = page.locator('#scope-select');
    await expect(trigger).toBeEnabled();
    await trigger.click();
    // The closed trigger names the family too; the row lives in the open picker.
    const family = page.getByRole('dialog', { name: 'Choose a project or target' }).getByRole('button', { name: /Andromeda exposures.*2 rigs/ });
    await expect(trigger).toContainText('2 rigs');
    await expect(family).toHaveCount(1);
    await expect(page.locator('.selector-project-toggle', { hasText: 'Andromeda exposures' })).toHaveCount(1);
    await expect(page.getByRole('region', { name: 'Andromeda exposures on Redcat data' })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('picker-family.png') });
    // Each rig's block offers that rig's images; these catalogs hold no files,
    // so the rows are present but disabled.
    await expect(page.getByRole('region', { name: 'Andromeda exposures on Redcat data' }).getByRole('button', { name: /All images/ })).toBeDisabled();
    await expect(page.getByRole('region', { name: 'Andromeda exposures on C925 data' }).getByRole('button', { name: /All images/ })).toBeDisabled();
    expect(errors).toEqual([]);
  } finally {
    for (const slug of slugs) await request.delete(`/api/databases/${slug}`);
  }
});
