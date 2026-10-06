import { expect, test, type Page } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

/**
 * Layout problems on the page as drawn: controls and text whose boxes
 * overlap without one holding the other, boxes that clip their own content
 * vertically, and sideways scroll. Absolute overlays on the sky are checked
 * against each other, not against the sky they sit on.
 */
async function layoutProblems(page: Page, scope = '.director-page'): Promise<string[]> {
  return page.evaluate((scope) => {
    const root = document.querySelector(scope) ?? document.body;
    // The part of a box left on screen once every scrolling or clipping
    // ancestor and the window have cut it; null when nothing is left.
    const visibleBox = (el: Element) => {
      const box = el.getBoundingClientRect();
      let left = Math.max(box.left, 0);
      let top = Math.max(box.top, 0);
      let right = Math.min(box.right, window.innerWidth);
      let bottom = Math.min(box.bottom, window.innerHeight);
      for (let at: Element | null = el; at; at = at.parentElement) {
        const style = getComputedStyle(at);
        if (style.display === 'none' || style.visibility === 'hidden' || at.hasAttribute('hidden')) return null;
        if (at.tagName === 'DETAILS' && !(at as HTMLDetailsElement).open && at !== el && !el.closest('summary')) return null;
        if (at !== el && (style.overflowX !== 'visible' || style.overflowY !== 'visible')) {
          const clip = at.getBoundingClientRect();
          left = Math.max(left, clip.left); top = Math.max(top, clip.top);
          right = Math.min(right, clip.right); bottom = Math.min(bottom, clip.bottom);
        }
      }
      return right - left >= 1 && bottom - top >= 1 ? { left, top, right, bottom } : null;
    };
    const name = (el: Element) => {
      const label = el.getAttribute('aria-label') ?? (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40);
      const cls = typeof el.className === 'string' && el.className ? `.${el.className.trim().split(/\s+/).join('.')}` : '';
      return `${el.tagName.toLowerCase()}${cls} "${label}"`;
    };
    // Content scrolls under the sticky top on purpose; anything the top
    // covers is out of sight, not overlapped.
    const sticky = document.querySelector('.workspace-top');
    const stickyBottom = sticky ? sticky.getBoundingClientRect().bottom : 0;
    const items = [...root.querySelectorAll('button, input, select, textarea, a, [role="switch"], legend, h1, h2, h3, h4, label, p, small, .workspace-pill, .framing-stage-status > *, .framing-stage-scale')]
      .filter(el => !el.closest('svg'))
      .map(el => {
        let box = visibleBox(el);
        if (box && sticky && !sticky.contains(el)) box = box.bottom - Math.max(box.top, stickyBottom) >= 1 ? { ...box, top: Math.max(box.top, stickyBottom) } : null;
        return { el, box };
      })
      .filter((item): item is { el: Element; box: { left: number; top: number; right: number; bottom: number } } => item.box !== null);
    const problems: string[] = [];
    // Two boxes overlap when they share more than a pixel each way and
    // neither holds the other (a label holds its input).
    for (let i = 0; i < items.length; i += 1) {
      const a = items[i];
      for (let j = i + 1; j < items.length; j += 1) {
        const b = items[j];
        if (a.el.contains(b.el) || b.el.contains(a.el)) continue;
        const x = Math.min(a.box.right, b.box.right) - Math.max(a.box.left, b.box.left);
        const y = Math.min(a.box.bottom, b.box.bottom) - Math.max(a.box.top, b.box.top);
        if (x > 1 && y > 1) problems.push(`overlap: ${name(a.el)} × ${name(b.el)} (${Math.round(x)}×${Math.round(y)}px)`);
      }
    }
    // Controls stacked with no room between them read as one clipped into
    // the next: one above the other, sharing some width, under 4px apart.
    const controls = items.filter(item => item.el.matches('button, input, select, textarea, [role="switch"]'));
    for (const a of controls) {
      for (const b of controls) {
        if (a === b || a.el.contains(b.el) || b.el.contains(a.el)) continue;
        const gap = b.box.top - a.box.bottom;
        const shared = Math.min(a.box.right, b.box.right) - Math.max(a.box.left, b.box.left);
        if (gap > -1 && gap < 4 && shared > 1) problems.push(`too close: ${name(a.el)} above ${name(b.el)} (${Math.round(gap)}px)`);
      }
    }
    // A box that cuts its own content off at the bottom.
    for (const el of root.querySelectorAll('*')) {
      if (el.closest('svg') || el.matches('.framing-stage, .framing-stage *') || !visibleBox(el)) continue;
      const style = getComputedStyle(el);
      // Text kept for screen readers only sits in a 1px box on purpose.
      const screenReaderOnly = el.clientWidth <= 1 || el.clientHeight <= 1;
      if (!screenReaderOnly && ['hidden', 'clip'].includes(style.overflowY) && el.scrollHeight > el.clientHeight + 2) {
        problems.push(`clipped: ${name(el)} (${el.scrollHeight} > ${el.clientHeight})`);
      }
    }
    const page = document.querySelector('.app-main') ?? document.documentElement;
    if (page.scrollWidth > page.clientWidth + 1) problems.push(`sideways scroll: ${page.scrollWidth} > ${page.clientWidth}`);
    if (root.scrollWidth > root.clientWidth + 1) problems.push(`sideways scroll in ${scope}: ${root.scrollWidth} > ${root.clientWidth}`);
    return problems;
  }, scope);
}

test('planning tabs lay out without overlaps or clipping', async ({ page, request }) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1440, height: 1000 });
  const slugs: string[] = [];
  for (const [index, name] of ['RedCat data', 'C925 data', 'Askar data'].entries()) {
    const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'layout-'));
    const dbPath = path.join(root, 'catalog.sqlite');
    const db = new Database(dbPath);
    applyRealSchema(db);
    db.prepare('INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,?,?,?,100,30,1,0,?)').run('p', 'Ha', 'Ha', randomUUID());
    if (index === 0) {
      db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'p','Medusa Nebula','',1,1,0,0,?)").run(randomUUID());
      db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES(1,?,1,7.48,13.25,2,1,?)').run('Medusa Nebula', randomUUID());
      db.prepare('INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,?,300,40,1,1,1,1,1,?)').run('p', randomUUID());
      db.prepare('INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(1,1,1,1750000000,?,1,?,?,1,?)').run('Ha', JSON.stringify({ FileName: 'x.fits' }), 'p', randomUUID());
    }
    db.close();
    const slug = `layout-${randomUUID().slice(0, 8)}`;
    expect((await request.post('/api/databases', { data: { name, slug, db_path: dbPath, image_dirs: [root] } })).ok()).toBeTruthy();
    slugs.push(slug);
  }
  try {
    await page.goto('/#/?db=parked');
    await expect(page.locator('[data-project-key]').filter({ hasText: 'Medusa Nebula' })).toHaveCount(1);
    const profiles = (await (await request.get('/api/director/v1/rigs/profiles')).json()).data;
    for (const [index, entry] of profiles.entries()) {
      const view = (await (await request.get(`/api/director/v1/catalogs/${entry.catalog_slug}/rig/profile`)).json()).data;
      const optics = { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: [300, 2350, 749][index % 3], aperture_mm: 61, rotation: { mode: 'manual', angle_degrees: 0 } };
      await request.put(`/api/director/v1/catalogs/${entry.catalog_slug}/rig/profile`, { data: { expected_revision: view.profile.revision, optics: { value: optics, source: { kind: 'manual' } }, site: null, horizon: null, sky_quality: null, limits: { value: view.profile.limits.value, source: { kind: 'manual' } }, peer_id: null } });
    }
    await page.reload();
    await page.locator('[data-project-key]').filter({ hasText: 'Medusa Nebula' }).getByRole('button', { name: /^Open the .* plan$/ }).first().click();
    await expect(page.getByTestId('framing-panel-source')).toContainText('Size from');
    const tab = (name: string) => page.getByRole('tab', { name: new RegExp(`^${name}`) });
    const saveBar = page.getByRole('region', { name: 'Unsaved changes' });
    // A plan with an objective and three rigs on, one framed separately.
    await tab('Exposures').click();
    await page.getByRole('button', { name: 'Add objective' }).click();
    await saveBar.getByRole('button', { name: 'Save changes' }).click();
    await expect(saveBar).toContainText('All changes saved');
    await tab('Framing').click();
    for (const rig of ['C925 data', 'Askar data']) await page.getByRole('switch', { name: `${rig} on` }).click();
    await page.getByRole('group', { name: 'Askar data' }).getByRole('button', { name: 'Frame separately' }).click();
    await page.getByLabel('Askar data columns').fill('2');
    await saveBar.getByRole('button', { name: 'Save changes' }).click();
    await expect(saveBar).toContainText('All changes saved');
    // Unsaved edits, so the save bar is up too; the separate framing's editor
    // stays open from above.
    await expect(page.getByRole('group', { name: 'Askar data' }).getByRole('button', { name: 'Done' })).toBeVisible();
    await page.getByLabel('Askar data camera angle degrees').fill('30');
    const found: string[] = [];
    const sizes = [[1440, 1000], [1280, 800], [1100, 900], [1024, 768], [900, 900], [375, 812]] as const;
    for (const tab of ['Framing', 'Exposures', 'Rigs', 'Priority and defaults']) {
      await page.getByRole('tab', { name: new RegExp(`^${tab}`) }).click();
      for (const [width, height] of sizes) {
        await page.setViewportSize({ width, height });
        await page.waitForTimeout(300);
        for (const problem of await layoutProblems(page)) found.push(`${tab} @${width}×${height}: ${problem}`);
        if (tab === 'Framing') {
          await page.getByLabel('Position angle degrees').scrollIntoViewIfNeeded();
          for (const problem of await layoutProblems(page)) found.push(`${tab} (angle in view) @${width}×${height}: ${problem}`);
          await page.locator('.app-main').evaluate(el => el.scrollTo(0, 0));
        }
      }
      await page.setViewportSize({ width: 1440, height: 1000 });
    }
    // The activation preview, over the page.
    page.once('dialog', confirm => void confirm.accept());
    await saveBar.getByRole('button', { name: 'Discard changes' }).click();
    await page.getByRole('region', { name: 'Activation due' }).getByRole('button', { name: 'Activate…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Activate on the rigs' });
    await dialog.getByRole('button', { name: 'Preview' }).click();
    await expect(dialog.getByLabel('RedCat data activation')).toBeVisible();
    for (const [width, height] of sizes) {
      await page.setViewportSize({ width, height });
      await page.waitForTimeout(300);
      for (const problem of await layoutProblems(page, '[role="dialog"]')) found.push(`Activation @${width}×${height}: ${problem}`);
    }
    expect([...new Set(found)], 'layout problems').toEqual([]);
  } finally {
    for (const slug of slugs) await request.delete(`/api/databases/${slug}`);
  }
});
