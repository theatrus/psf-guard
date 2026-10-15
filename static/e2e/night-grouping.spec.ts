import { expect, test, type APIRequestContext } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import { fixtureDbPath, fixtureImageDir, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

// Images' Date groups on a site at UTC-4, whose evenings run past 00:00 UTC:
// each night is one group, under the date its evening began, split where the
// server says the catalog's nights split.

test.use({ timezoneId: 'America/New_York' });

const HOUR = 60 * 60;
/** 19:30 EDT on 2026-10-12, which is 23:30 UTC. */
const FIRST_EVENING = Date.UTC(2026, 9, 12, 23, 30) / 1000;

/** Two nights of Ha and OIII, every half hour from 19:30 to 04:00 local. */
function twoNights(): { at: number; filter: string }[] {
  const frames: { at: number; filter: string }[] = [];
  for (const night of [0, 1]) {
    for (let step = 0; step < 18; step += 1) {
      frames.push({ at: FIRST_EVENING + night * 24 * HOUR + step * HOUR / 2, filter: step % 2 ? 'OIII' : 'Ha' });
    }
  }
  return frames;
}

/** A copy of the fixture catalog holding only these frames, each with one
 *  real frame's headers. */
function catalogOf(frames: { at: number; filter: string }[]): string {
  const databasePath = path.join(tmpBase(), `night-grouping-${randomUUID()}.sqlite`);
  fs.copyFileSync(fixtureDbPath(), databasePath);
  const database = new Database(databasePath);
  try {
    const existing = database.prepare('SELECT metadata FROM acquiredimage WHERE projectId=1 LIMIT 1').get() as {
      metadata: string;
    };
    database.exec('DELETE FROM acquiredimage');
    const insert = database.prepare(
      'INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata) VALUES(?,1,1,?,?,0,?)',
    );
    frames.forEach((frame, index) => insert.run(1000 + index, frame.at, frame.filter, JSON.stringify({
      ...JSON.parse(existing.metadata),
      ExposureStartTime: new Date(frame.at * 1000).toISOString(),
      FilterName: frame.filter,
    })));
  } finally {
    database.close();
  }
  return databasePath;
}

async function register(request: APIRequestContext, databasePath: string): Promise<string> {
  await resetDatabases(request);
  const registered = await request.post('/api/databases', {
    data: { name: 'Night grouping', slug: `nights-${randomUUID().slice(0, 8)}`, db_path: databasePath, image_dirs: [fixtureImageDir()] },
  });
  expect(registered.ok(), await registered.text()).toBeTruthy();
  const dbId = (await registered.json()).data.id as string;
  await waitForCacheReady(request, dbId).catch(() => {});
  return dbId;
}

test('Date groups are observing nights west of Greenwich', async ({ page, request }, testInfo) => {
  test.setTimeout(90_000);
  const dbId = await register(request, catalogOf(twoNights()));

  // The server splits this catalog's nights in its quiet afternoon.
  const stats = await request.get(`/api/db/${encodeURIComponent(dbId)}/stats/overall`);
  expect(stats.ok()).toBeTruthy();
  expect((await stats.json()).data.night_boundary_seconds).toBe(16 * HOUR);

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&grouping=date`);
  const headings = page.locator('.filter-header h3');
  // One night per group, newest first, under the evening's date, though
  // each night crosses 00:00 UTC. The project splits each by exposure,
  // which here means by filter.
  await expect(headings).toHaveText(
    [/^2026-10-13 · Ha/, /^2026-10-13 · OIII/, /^2026-10-12 · Ha/, /^2026-10-12 · OIII/],
    { timeout: 30_000 },
  );
  await expect(page.locator('.filter-header .stat-total')).toHaveText(Array(4).fill('9 images'));
  await page.screenshot({ path: testInfo.outputPath('date-groups-1440.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(headings.first()).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('date-groups-390.png') });

  // Filter + Date keeps each filter's night together too.
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&grouping=both`);
  await expect(headings).toHaveText(
    [/^Ha - 2026-10-12/, /^Ha - 2026-10-13/, /^OIII - 2026-10-12/, /^OIII - 2026-10-13/],
    { timeout: 30_000 },
  );

  // A link from before, naming the UTC date 2026-10-14, opens the night
  // that holds that date's frames, the small hours of the second night,
  // and no other.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&grouping=date&expanded=2026-10-14`);
  await expect(headings).toHaveCount(4, { timeout: 30_000 });
  await expect(page.locator('.filter-toggle')).toHaveText(['▼', '▼', '▶', '▶']);
});
