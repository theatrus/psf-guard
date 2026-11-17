import { expect, test, type Locator, type Page } from '@playwright/test';
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
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'framing-sky-'));
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

/** Whether the page scrolls sideways at this width, or a control of the
 *  Sky in view group runs off the window. */
async function sidewaysScroll(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const found: string[] = [];
    const root = document.documentElement;
    if (root.scrollWidth > root.clientWidth + 1) found.push(`page ${root.scrollWidth} > ${root.clientWidth}`);
    const box = document.querySelector('.sky-in-view');
    if (box) {
      if (box.scrollWidth > box.clientWidth + 1) found.push(`group ${box.scrollWidth} > ${box.clientWidth}`);
      for (const control of box.querySelectorAll('input, button, label, p')) {
        const at = control.getBoundingClientRect();
        if (at.width > 0 && (at.left < -1 || at.right > root.clientWidth + 1)) found.push(`${control.tagName} at ${Math.round(at.left)}..${Math.round(at.right)} of ${root.clientWidth}`);
      }
    }
    return found;
  });
}

/** Where an element sits in the window, to the pixel. */
const boxOf = async (element: Locator) => {
  const box = (await element.boundingBox())!;
  return [Math.round(box.x), Math.round(box.y), Math.round(box.width), Math.round(box.height)];
};

test('Framing covers the sky the rig cannot shoot, for the month, the year and a season, and says where the target stands', async ({ page, request }, testInfo) => {
  test.setTimeout(240_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const placed = `framing-sky-${randomUUID().slice(0, 8)}`;
  const unplaced = `framing-sky-${randomUUID().slice(0, 8)}`;
  const [placedPlan, unplacedPlan] = [randomUUID(), randomUUID()];
  try {
    await addRig(request, 'Sky placed', placed, planDatabase('Sky placed', placedPlan), { latitude_degrees: 34.2, longitude_degrees: -118.3 });
    // A second rig with no location of its own and no Planning site.
    const unplacedDb = planDatabase('Sky unplaced', unplacedPlan);
    const added = await request.post('/api/databases', { data: { name: 'Sky unplaced', slug: unplaced, db_path: unplacedDb, image_dirs: [path.dirname(unplacedDb)] } });
    expect(added.ok(), await added.text()).toBeTruthy();
    expect((await request.get('/api/director/v1/plans')).ok()).toBeTruthy();

    await page.setViewportSize({ width: 1280, height: 900 });
    // Every request for the picture or for a place's nights.
    const posts: Array<{ span?: string; minimum_minutes?: number; measured?: boolean; priority?: string; places?: unknown[] }> = [];
    page.on('request', sent => { if (/\/sky\/visible-(region|places)$/.test(sent.url())) posts.push(JSON.parse(sent.postData() ?? '{}')); });
    const pictures = () => posts.filter(body => !('places' in body));
    // An address from when the plan had a Sky tab opens Framing.
    await page.goto(`/#/plan?plan=${placedPlan}&planTab=sky`);
    await expect(page.getByRole('tablist', { name: 'Plan sections' }).getByRole('tab')).toHaveText(['Framing', 'Exposures', 'Rigs', 'Priority and defaults']);
    await expect(page.getByRole('tab', { name: 'Framing' })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByLabel('Target name')).toHaveValue('Sky placed target', { timeout: 30_000 });

    // The overlay is on from the start, for the plan's rig, over the stage.
    const stage = page.getByTestId('framing-stage');
    const layer = page.getByTestId('sky-in-view-layer');
    const group = page.getByTestId('sky-in-view');
    const words = page.getByTestId('sky-in-view-status');
    await expect(layer).toHaveAttribute('data-nights', '31', { timeout: 60_000 });
    await expect(layer.locator('image')).toHaveAttribute('href', /^data:image\/png;base64,.{100,}/);
    await expect(page.getByRole('button', { name: 'Sky in view' })).toHaveAttribute('aria-pressed', 'true');
    // The target near the pole is in view every night on any date, and
    // the words say so without zooming out.
    await expect(words).toHaveText('The target is in view on all of the 31 nights counted.');
    await expect(page.getByTestId('sky-in-view-note')).toHaveCount(0);
    expect(pictures()).toEqual([{ rig: expect.any(String), span: 'now' }]);
    await page.screenshot({ path: testInfo.outputPath('framing-sky-now.png') });

    // Zoomed out, the lines show where the limits are.
    for (let step = 0; step < 16; step += 1) await page.getByRole('button', { name: 'Zoom out' }).click();
    await expect(layer.locator('.sky-in-view-line.sky-in-view-any')).toHaveAttribute('d', /^M[\d. -]+L/);
    // Its edge runs near the southern limit, below this view of the pole,
    // so the shares above give where it is rather than the drawn line.

    // The controls are folded behind one line.
    await group.locator('summary').click();
    await expect(page.getByTestId('sky-in-view-legend')).toContainText('Sky placed: in view 30 min or more · 31 nights from tonight, ');
    await expect(page.getByTestId('sky-in-view-shares')).toContainText(/In view every night: \d+% of the sky/);
    // Nothing in the group is read out as it changes.
    expect(await group.locator('[role="status"], [aria-live]').count()).toBe(0);

    // Year: every night counted, one a week measured. The sky left clear
    // is the sky in view on at least 7 nights of the year, so the cover is
    // weighed against 7, and the target near the pole is clear, with no note.
    await group.getByRole('radio', { name: 'Year' }).click();
    await expect(layer).toHaveAttribute('data-nights', '7', { timeout: 120_000 });
    await expect(words).toHaveText('The target is in view on all of the 365 nights of the year.');
    await expect(page.getByTestId('sky-in-view-note')).toHaveCount(0);
    await expect(page.getByTestId('sky-in-view-legend')).toContainText('365 nights from tonight, ');
    await expect(page.getByTestId('sky-in-view-shares')).toContainText(/^In view on 7 nights or more: \d+% of the sky, declination −\d+° to \+90°\./);
    await expect(group).toContainText('Clear sky is in view on at least 7 of them, in total and not in a row.');
    await expect(group).toContainText('No night7 nights or more');
    // Its edge runs near the southern limit, below this view of the pole,
    // so the shares above give where it is rather than the drawn line.
    await expect(page.getByTestId('sky-in-view-shares')).toContainText(/On at least one night: \d+% of the sky, declination −3[01]° to \+90°\./);
    // A picture chosen here is a person's request; the one drawn on
    // opening was the page's own.
    expect(pictures().at(-1)).toEqual({ rig: expect.any(String), span: 'year', priority: 'interactive' });

    // Season: a month of the year at a time under a slider. It asks once,
    // for the year with its measured nights.
    await group.getByRole('radio', { name: 'Season' }).click();
    const slider = group.getByRole('slider', { name: 'Time of year' });
    await expect(slider).toBeEnabled({ timeout: 120_000 });
    await expect(slider).toHaveValue('0');
    await expect(slider).toHaveAttribute('aria-valuetext', /^31 nights from \S/);
    await expect(layer).toHaveAttribute('data-nights', '31');
    await expect(words).toHaveText('The target is in view on all of the 31 nights counted.');
    expect(pictures().at(-1)).toEqual({ rig: expect.any(String), span: 'year', measured: true, priority: 'interactive' });
    expect(pictures()).toHaveLength(3);
    const firstValue = (await slider.getAttribute('aria-valuetext')) ?? '';
    const firstShares = (await page.getByTestId('sky-in-view-shares').textContent()) ?? '';
    const firstCover = await layer.locator('image').getAttribute('href');
    const legend = (await page.getByTestId('sky-in-view-legend').textContent()) ?? '';
    const beforeSliding = posts.length;
    // Half a year on, a week a key press: other sky is covered, and
    // neither the stage nor the slider moves under the hand.
    await slider.focus();
    await slider.scrollIntoViewIfNeeded();
    await page.waitForTimeout(200);
    const [stageAt, sliderAt] = [await boxOf(stage), await boxOf(slider)];
    for (let week = 0; week < 26; week += 1) {
      await slider.press('ArrowRight');
      if (week % 6 === 0) {
        expect(await boxOf(stage), `stage after ${week + 1} weeks`).toEqual(stageAt);
        expect(await boxOf(slider), `slider after ${week + 1} weeks`).toEqual(sliderAt);
      }
    }
    await expect(slider).toHaveValue('26');
    await expect(slider).not.toHaveAttribute('aria-valuetext', firstValue);
    await expect(page.getByTestId('sky-in-view-shares')).not.toHaveText(firstShares);
    await expect.poll(async () => layer.locator('image').getAttribute('href')).not.toBe(firstCover);
    // What stays the same along the slider does not change.
    await expect(page.getByTestId('sky-in-view-legend')).toHaveText(legend);
    await expect(words).toHaveText('The target is in view on all of the 31 nights counted.');
    expect(await boxOf(stage)).toEqual(stageAt);
    expect(await boxOf(slider)).toEqual(sliderAt);
    await page.screenshot({ path: testInfo.outputPath('framing-sky-season.png') });
    // Home and End reach the ends, and a page is four weeks.
    await slider.press('End');
    await expect(slider).toHaveValue('48');
    await slider.press('Home');
    await expect(slider).toHaveValue('0');
    await slider.press('PageUp');
    await expect(slider).toHaveValue('4');
    expect(posts.length).toBe(beforeSliding);
    // Year reads the answer Season fetched: going back asks for nothing.
    await group.getByRole('radio', { name: 'Year' }).click();
    await expect(layer).toHaveAttribute('data-nights', '7');
    await group.getByRole('radio', { name: 'Season' }).click();
    await expect(slider).toHaveValue('4');
    expect(posts.length).toBe(beforeSliding);

    // A phone: nothing scrolls sideways, and the stage stays put while
    // the slider moves.
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(400);
    expect(await sidewaysScroll(page), '390px').toEqual([]);
    // Measured with the slider already focused and in sight, so the only
    // thing that could move them is what the slider changes.
    await slider.focus();
    await slider.scrollIntoViewIfNeeded();
    await page.waitForTimeout(200);
    const [phoneStage, phoneSlider] = [await boxOf(stage), await boxOf(slider)];
    for (let week = 0; week < 8; week += 1) await slider.press('ArrowRight');
    await expect(slider).toHaveValue('12');
    expect(await boxOf(stage)).toEqual(phoneStage);
    expect(await boxOf(slider)).toEqual(phoneSlider);
    await page.setViewportSize({ width: 320, height: 700 });
    await page.waitForTimeout(400);
    expect(await sidewaysScroll(page), '320px').toEqual([]);
    await page.setViewportSize({ width: 1280, height: 900 });

    // A target the rig can never reach reads so in words at a tight zoom,
    // where the whole view is one state.
    await group.getByRole('radio', { name: 'Now' }).click();
    await page.getByLabel('Declination degrees').fill('-60');
    await page.getByLabel('Declination degrees').press('Tab');
    await page.getByRole('button', { name: 'Center view on target' }).click();
    for (let step = 0; step < 16; step += 1) await page.getByRole('button', { name: 'Zoom in' }).click();
    await expect(words).toHaveText('The target is never in view on the 31 nights counted.', { timeout: 30_000 });
    await expect(words).not.toHaveAttribute('data-stale', 'true');
    await expect(page.getByTestId('sky-in-view-note')).toHaveText(['Target never in view']);
    // Nor on any night of the year: Year notes it too.
    await group.getByRole('radio', { name: 'Year' }).click();
    await expect(words).toHaveText('The target is never in view on the 365 nights of the year.', { timeout: 60_000 });
    await expect(page.getByTestId('sky-in-view-note')).toHaveText(['Target never in view']);
    await group.getByRole('radio', { name: 'Now' }).click();
    await expect(words).toHaveText('The target is never in view on the 31 nights counted.', { timeout: 30_000 });
    // No count for a place was a person's request, and typing the target
    // asked once, when it rested.
    expect(posts.filter(body => 'places' in body).every(body => !('priority' in body))).toBe(true);
    await page.screenshot({ path: testInfo.outputPath('framing-sky-never.png') });

    // Switched off, the layer goes and nothing more is asked; the choice
    // is kept for the next visit.
    await page.getByRole('button', { name: 'Sky in view' }).click();
    await expect(layer).toHaveCount(0);
    await expect(words).toHaveText('Hidden');
    await expect(page.getByTestId('sky-in-view-note')).toHaveCount(0);
    await page.reload();
    await expect(page.getByLabel('Target name')).toHaveValue('Sky placed target', { timeout: 30_000 });
    await expect(page.getByRole('button', { name: 'Sky in view' })).toHaveAttribute('aria-pressed', 'false');
    await expect(page.getByTestId('sky-in-view-layer')).toHaveCount(0);
    await page.getByTestId('sky-in-view').getByLabel('Show on the sky').check();
    await expect(page.getByTestId('sky-in-view-layer')).toHaveAttribute('data-nights', '31', { timeout: 60_000 });

    // The overlay follows the rig Visibility shows, and gives its name. A
    // plan whose own rig has no site is shown for a rig that has one.
    await page.goto(`/#/plan?plan=${unplacedPlan}`);
    await expect(page.getByLabel('Target name')).toHaveValue('Sky unplaced target', { timeout: 30_000 });
    await expect(page.getByTestId('sky-in-view').locator('legend')).toHaveText('Sky in view · Sky placed', { timeout: 30_000 });
    // With no rig that has a site, the words explain what to set, and
    // nothing is drawn.
    expect((await request.delete(`/api/databases/${placed}`)).ok()).toBeTruthy();
    await page.reload();
    await expect(page.getByLabel('Target name')).toHaveValue('Sky unplaced target', { timeout: 30_000 });
    await expect(page.getByTestId('sky-in-view-status')).toContainText('has no location. Choose a Planning site in Rig setup.', { timeout: 30_000 });
    await expect(page.getByTestId('sky-in-view').locator('legend')).toHaveText('Sky in view · Sky unplaced');
    await expect(page.getByTestId('sky-in-view').getByRole('button', { name: 'Open Rig setup' })).toBeVisible();
    await expect(page.getByTestId('sky-in-view-layer')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    for (const slug of [placed, unplaced]) await request.delete(`/api/databases/${slug}`);
  }
});
