import { expect, test, type Page } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';
import { addRig } from './nightsFixtures';

/** A rig database with one Target Scheduler project and its target, at
 *  declination +85°: enough for a plan. From 34° north with a 25° minimum
 *  that place never sets below the limit, so it is in view on every night
 *  of any month and year, whatever the date the suite runs on. No frame
 *  has been taken. */
function planDatabase(name: string, guid: string): string {
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'plan-sky-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'current-profile',?,'',1,1,0,0,?)").run(`${name} project`, guid);
  db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES(1,?,1,0.712313,85.0,2,1,?)').run(`${name} target`, randomUUID());
  db.prepare("INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,'current-profile','Ha','Ha',100,30,1,0,?)").run(randomUUID());
  db.prepare("INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,'current-profile',300,40,0,0,1,1,1,?)").run(randomUUID());
  db.close();
  return dbPath;
}

/** Sideways scroll of the page or of the Sky tab at this width, and any
 *  control that runs off the window. */
async function sidewaysScroll(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const found: string[] = [];
    const root = document.documentElement;
    if (root.scrollWidth > root.clientWidth + 1) found.push(`page ${root.scrollWidth} > ${root.clientWidth}`);
    for (const selector of ['.plan-sky', '.plan-sky-controls', '.plan-sky-side', '.plan-sky-key']) {
      const box = document.querySelector(selector);
      if (!box) continue;
      if (box.scrollWidth > box.clientWidth + 1) found.push(`${selector} ${box.scrollWidth} > ${box.clientWidth}`);
      for (const control of box.querySelectorAll('select, input, button, label')) {
        const at = control.getBoundingClientRect();
        if (at.width > 0 && (at.left < -1 || at.right > root.clientWidth + 1)) found.push(`${selector} ${control.tagName} at ${Math.round(at.left)}..${Math.round(at.right)} of ${root.clientWidth}`);
      }
    }
    return found;
  });
}

test("a plan's Sky tab shades the sky its rig can shoot, for the month and the year, and sends a picked place to Framing", async ({ page, request }, testInfo) => {
  test.setTimeout(240_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const placed = `plan-sky-${randomUUID().slice(0, 8)}`;
  const unplaced = `plan-sky-${randomUUID().slice(0, 8)}`;
  const [placedPlan, unplacedPlan] = [randomUUID(), randomUUID()];
  let bare: string | null = null;
  try {
    await addRig(request, 'Sky placed', placed, planDatabase('Sky placed', placedPlan), { latitude_degrees: 34.2, longitude_degrees: -118.3 });
    // A second rig with no location of its own and no Planning site.
    const unplacedDb = planDatabase('Sky unplaced', unplacedPlan);
    const added = await request.post('/api/databases', { data: { name: 'Sky unplaced', slug: unplaced, db_path: unplacedDb, image_dirs: [path.dirname(unplacedDb)] } });
    expect(added.ok(), await added.text()).toBeTruthy();
    expect((await request.get('/api/director/v1/plans')).ok()).toBeTruthy();
    // A plan with no database and so no rig.
    const created = await request.post('/api/director/v1/projects', { data: { id: randomUUID(), name: 'Sky bare plan' } });
    expect(created.ok(), await created.text()).toBeTruthy();
    bare = (await created.json()).data.id as string;

    await page.setViewportSize({ width: 1440, height: 1000 });
    // Every request for the picture or for a place's nights.
    const posts: Array<{ span?: string; minimum_minutes?: number; places?: unknown[] }> = [];
    page.on('request', sent => { if (/\/sky\/visible-(region|places)$/.test(sent.url())) posts.push(JSON.parse(sent.postData() ?? '{}')); });
    const pictures = () => posts.filter(body => !('places' in body));
    await page.goto(`/#/plan?plan=${placedPlan}`);
    // The tab sits after Framing and asks for nothing until it is opened.
    await expect(page.getByRole('tablist', { name: 'Plan sections' }).getByRole('tab')).toHaveText(['Framing', 'Sky', 'Exposures', 'Rigs', 'Priority and defaults']);
    await expect(page.getByLabel('Target name')).toHaveValue('Sky placed target', { timeout: 30_000 });
    expect(posts).toEqual([]);
    await page.getByRole('tab', { name: 'Sky' }).click();
    await expect(page).toHaveURL(/planTab=sky/);
    const tab = page.getByRole('region', { name: 'Sky in view' });
    const legend = tab.getByRole('status');

    // Now: the month, shaded, with both lines and the numbers in words.
    await expect(legend).toContainText('Sky placed: in view 30 min or more · 31 nights from tonight, ', { timeout: 120_000 });
    await expect(legend).toContainText('flat horizon at 25°');
    await expect(legend).toContainText(/In view every night: \d+% of the sky, declination [−+]\d+° to \+90°\. On at least one night: \d+% of the sky, declination −3[01]° to \+90°\./);
    await expect(tab.getByRole('radio', { name: 'Now' })).toHaveAttribute('aria-checked', 'true');
    const shade = tab.locator('.sky-visible-shade');
    await expect(shade).toHaveAttribute('data-nights', '31');
    await expect(shade.locator('image')).toHaveAttribute('href', /^data:image\/png;base64,.{200,}/);
    const every = tab.locator('.sky-visible-line.sky-visible-every');
    await expect(every).toHaveAttribute('d', /^M[\d.,-]+L/);
    await expect(tab.locator('.sky-visible-line.sky-visible-any')).toHaveAttribute('d', /^M[\d.,-]+L/);
    await expect(tab.locator('.sky-visible-marks.sky-visible-every')).toHaveAttribute('d', /^M/);
    const map = tab.locator('.sky-map');
    await expect(map).toHaveAttribute('aria-label', /^Map of the sky Sky placed: in view 30 min or more.*In view every night: \d+% of the sky/);
    await expect(tab.getByLabel('How to read the map')).toContainText('No night');
    await expect(tab.getByLabel('How to read the map')).toContainText('Every night');
    // The plan's target is marked and counted.
    await expect(tab.locator('.sky-mark-target')).toHaveAttribute('data-mark', 'Sky placed target');
    // The target is circumpolar for this rig: in view every night on any date.
    await expect(tab.getByTestId('plan-sky-target')).toContainText('In view on 31 of the 31 nights counted. Inside the every-night line.');
    // One request counted the target's nights, for the place itself.
    expect(posts.filter(body => 'places' in body)).toHaveLength(1);
    expect(pictures()).toEqual([{ rig: expect.any(String), span: 'now' }]);
    await page.screenshot({ path: testInfo.outputPath('plan-sky-now.png') });

    // Year: a night a week; every right ascension has its season.
    await tab.getByRole('radio', { name: 'Year' }).click();
    await expect(legend).toContainText('one night a week for a year', { timeout: 120_000 });
    await expect(shade).toHaveAttribute('data-nights', '53');
    await expect(legend).toContainText(/On at least one night: \d+% of the sky, declination −3[01]° to \+90°\./);
    await expect(tab.getByTestId('plan-sky-target')).toContainText('In view on 53 of the 53 nights counted over the year. Inside the every-night line.');
    await expect(tab.getByLabel('How to read the map')).toContainText('Year counts one night a week.');
    expect(pictures().at(-1)).toEqual({ rig: expect.any(String), span: 'year' });
    // The globe draws them too, and turning the map asks for nothing.
    await tab.getByRole('radio', { name: 'Globe' }).click();
    await expect(every).toHaveAttribute('d', /^M[\d.,-]+L/);
    await expect(shade.locator('image')).toHaveAttribute('href', /^data:image\/png;base64,/);
    await tab.getByRole('radio', { name: 'Flat' }).click();
    await tab.getByRole('radio', { name: 'Now' }).click();
    await expect(legend).toContainText('31 nights');
    expect(pictures()).toHaveLength(2);

    // A longer minimum, typed a key at a time, is asked for once.
    await tab.getByLabel('Minimum minutes').click();
    await tab.getByLabel('Minimum minutes').press('ControlOrMeta+a');
    await tab.getByLabel('Minimum minutes').pressSequentially('240', { delay: 60 });
    await expect(legend).toContainText('in view 4 h or more', { timeout: 60_000 });
    expect(pictures().slice(2).map(body => body.minimum_minutes)).toEqual([240]);
    await tab.getByRole('button', { name: "Use rig's 30 min" }).click();
    await expect(legend).toContainText('in view 30 min or more');
    // Nothing is fetched again while the tab sits open.
    const settled = posts.length;
    await page.waitForTimeout(1500);
    expect(posts.length).toBe(settled);

    // Phone widths: the controls wrap and nothing scrolls sideways.
    for (const [width, height] of [[390, 844], [320, 700]] as const) {
      await page.setViewportSize({ width, height });
      await page.waitForTimeout(300);
      expect(await sidewaysScroll(page), `${width}px`).toEqual([]);
    }
    await page.setViewportSize({ width: 1440, height: 1000 });

    // A click on the map picks the place; nothing changes until Frame this.
    await expect(tab.getByRole('group', { name: 'Picked place' })).toHaveCount(0);
    await map.scrollIntoViewIfNeeded();
    const box = (await map.boundingBox())!;
    await page.mouse.click(box.x + box.width * 0.5, box.y + box.height * 0.3);
    const card = tab.getByRole('group', { name: 'Picked place' });
    // Whatever the date, the place is counted for itself and placed among the lines.
    await expect(card).toContainText(/In view on \d+ of the 31 nights counted\. (Inside the every-night line|Between the lines: in view on some nights|Outside both lines: never in view for long enough)\./);
    await expect(tab.locator('.sky-mark-pick')).toHaveCount(1);
    const pickedName = (await card.locator('strong').textContent())!;
    expect(pickedName).toMatch(/^\d+h \d+m, [+−]\d+° \d+′$/);
    await expect(page.getByRole('region', { name: 'Unsaved changes' })).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath('plan-sky-picked.png') });
    await card.getByRole('button', { name: 'Frame this' }).click();
    // Framing opens with the place as its target: an unsaved edit, with Undo.
    await expect(page.getByRole('tab', { name: /^Framing/ })).toHaveAttribute('aria-selected', 'true');
    await expect(page).toHaveURL(/planTab=framing/);
    await expect(page.getByLabel('Target name')).toHaveValue(pickedName);
    await expect(page.getByText(`Moved the target to ${pickedName}.`)).toBeVisible();
    await expect(page.getByRole('region', { name: 'Unsaved changes' })).toContainText('Framing');
    // The Sky tab marks the target where Framing now has it.
    await page.getByRole('tab', { name: 'Sky' }).click();
    await expect(tab.locator('.sky-mark-target')).toHaveAttribute('data-mark', pickedName);
    // Undo in Framing puts the old target back, there and on the Sky tab.
    await page.getByRole('tab', { name: /^Framing/ }).click();
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(page.getByLabel('Target name')).toHaveValue('Sky placed target');
    await expect(page.getByRole('region', { name: 'Unsaved changes' })).toHaveCount(0);
    await page.getByRole('tab', { name: 'Sky' }).click();
    await expect(tab.locator('.sky-mark-target')).toHaveAttribute('data-mark', 'Sky placed target');

    // A rig with no site: the legend explains it and nothing is shaded.
    await page.goto(`/#/plan?plan=${unplacedPlan}&planTab=sky`);
    const noSite = page.getByRole('region', { name: 'Sky in view' });
    await expect(noSite.getByRole('status')).toContainText('has no location. Choose a Planning site in Rig setup.', { timeout: 30_000 });
    await expect(noSite.getByRole('button', { name: 'Open Rig setup' })).toBeVisible();
    await expect(noSite.locator('.sky-map')).toBeVisible();
    await expect(noSite.locator('.sky-visible-shade')).toHaveCount(0);
    await expect(noSite.locator('.sky-visible')).toHaveCount(0);

    // A plan with no rig: the tab says so and leads to Rigs.
    await page.goto(`/#/plan?plan=${bare}&planTab=sky`);
    const noRig = page.getByRole('region', { name: 'Sky in view' });
    await expect(noRig.getByRole('status')).toContainText('This plan has no rig yet.', { timeout: 30_000 });
    await expect(noRig.locator('.sky-map')).toHaveCount(0);
    await noRig.getByRole('button', { name: 'Add a rig on the Rigs tab' }).click();
    await expect(page.getByRole('tab', { name: 'Rigs' })).toHaveAttribute('aria-selected', 'true');

    // The Sky view is coverage and Live again: no Visible sky control.
    await page.goto('/#/sky');
    await expect(page.locator('.sky-page')).toBeVisible({ timeout: 15_000 });
    await expect(page.getByRole('button', { name: 'Visible sky' })).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    for (const slug of [placed, unplaced]) await request.delete(`/api/databases/${slug}`);
    if (bare) await request.post(`/api/director/v1/projects/${bare}/deletion`, { data: { digests: {} } });
  }
});
