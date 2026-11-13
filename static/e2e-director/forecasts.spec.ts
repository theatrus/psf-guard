import { expect, test, type APIRequestContext } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { openSettings, selectSettingsSection } from '../e2e/helpers';
import { addRig, rigDatabase, shot } from './nightsFixtures';

/** A patio's horizon, as N.I.N.A. writes it: a 70° wall all round but for
 *  an opening to the south-east. */
const PATIO_HRZ = '# Patio\n0 70\n99 70\n100 20\n200 20\n201 70\n360 70\n';

async function json(response: Awaited<ReturnType<APIRequestContext['get']>>) {
  expect(response.ok(), await response.text()).toBeTruthy();
  return (await response.json()).data;
}

// The suite's server takes its forecasts from e2e-director/forecast-provider.mjs,
// whose nights cycle clear, overcast, then clear from 21:00 to 01:00.
test('a site with a forecast and a horizon file shapes its rigs\' nights', async ({ page, request }, testInfo) => {
  test.setTimeout(120_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const slug = `forecast-${randomUUID().slice(0, 6)}`;
  const siteName = `Patio ${randomUUID().slice(0, 4)}`;
  try {
    const rigId = await addRig(request, 'Patio rig', slug, rigDatabase([
      // The Heart rides in the north, under the wall's 70°.
      { name: 'Heart behind the wall', priority: 2, target: 'IC 1805', ra_hours: 2.5467, dec: 61.45, filters: [['Ha', 120]], customHorizon: true },
      { name: 'Heart on the minimum', priority: 1, target: 'IC 1805', ra_hours: 2.5467, dec: 61.45, filters: [['OIII', 120]] },
    ]), { latitude_degrees: 38.68, longitude_degrees: -121.18 });

    // Setup → Sites: the location, the patio's .hrz file and a forecast.
    await page.setViewportSize({ width: 1440, height: 1100 });
    await page.goto('/#/sky');
    await openSettings(page);
    await selectSettingsSection(page, 'sites');
    const sites = page.getByRole('region', { name: 'Sites', exact: true });
    await sites.getByLabel('New site name').fill(siteName);
    await sites.getByRole('button', { name: 'Add site' }).click();
    const card = sites.getByRole('region', { name: `Site ${siteName}` });
    await card.getByLabel('Latitude').fill('38.68');
    await card.getByLabel('Longitude (east +)').fill('-121.18');
    await card.getByLabel('Elevation').fill('70');
    await card.getByLabel('Upload Site horizon file').setInputFiles({ name: 'patio.hrz', mimeType: 'text/plain', buffer: Buffer.from(PATIO_HRZ) });
    await expect(card.getByTestId('horizon-summary')).toContainText('not saved yet');
    const sends = card.locator('.site-forecast-facts > div').filter({ hasText: 'Sends' });
    await expect(sends.getByRole('definition')).toHaveText('Nothing');
    await card.getByLabel('Forecast', { exact: true }).selectOption('open_meteo');
    await expect(sends.getByRole('definition')).toHaveText('Latitude and longitude, to about 1 km');
    await card.getByRole('button', { name: 'Save site' }).click();
    await expect(card.getByText(`Saved ${siteName}.`)).toBeVisible();
    const status = card.getByLabel('Forecast status');
    await expect(status.getByText('Fetched', { exact: true })).toBeVisible({ timeout: 30_000 });
    await expect(status.getByText('Covers until', { exact: true })).toBeVisible();
    await expect(status.getByText(/^Until /)).toBeVisible();
    await expect(status.getByRole('link', { name: 'Weather data by Open-Meteo.com (CC BY 4.0)' })).toBeVisible();
    await shot(card, 'site-forecast', name => testInfo.outputPath(name), false);
    await page.getByRole('button', { name: 'Close settings' }).click();

    // The rig plans at the site.
    const site = (await json(await request.get('/api/director/v1/sites'))).items.find((item: { name: string }) => item.name === siteName);
    const settings = await json(await request.get(`/api/director/v1/preferences/rig/${rigId}`));
    await json(await request.put(`/api/director/v1/preferences/rig/${rigId}`, { data: { ...settings, site_id: site.id } }));

    // The site's horizon hides the walled Heart from the plan that uses it.
    const planned = await json(await request.post('/api/director/v1/night-plans', { data: { rigs: [rigId], nights: 3 } }));
    const rig = planned.rigs[0];
    const project = (name: string) => rig.plans.find((plan: { name: string }) => plan.name === name).project_id;
    const hoursUp = (name: string) => Math.max(...rig.nights.map((night: { targets: Array<{ project_id: number; hours_up: number }> }) =>
      night.targets.find(target => target.project_id === project(name))!.hours_up));
    expect(hoursUp('Heart behind the wall')).toBe(0);
    expect(hoursUp('Heart on the minimum')).toBeGreaterThan(4);
    expect(rig.forecast.provider).toBe('open_meteo');

    // Nights: the forecast marks each night, with a cloud in its cell.
    await page.goto('/#/rigs?rigsTab=nights');
    const grid = page.getByTestId('nights-grid');
    await expect(grid).toBeVisible({ timeout: 30_000 });
    const cells = grid.getByRole('row').filter({ hasText: 'Patio rig' }).getByRole('button');
    await expect(cells).toHaveCount(7);
    await expect(cells.filter({ has: page.locator('.nights-cell-forecast') })).toHaveCount(7);
    await expect(grid.getByRole('button', { name: /not available by the forecast/ }).first()).toBeVisible();
    // The last night limited by cloud. The stand-in forecast starts 12 hours
    // back, so a night already under way before then is limited too, by the
    // hours the forecast lacks, and has no cloud to show. The first night
    // was such a night when this ran between about 13:30 and 20:00 UTC.
    const limited = grid.getByRole('button', { name: /limited by the forecast: cloud/ }).last();
    await expect(limited).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('nights-forecast-grid.png'), fullPage: true, animations: 'disabled' });

    // A limited night: the forecast's hours, its cloud under the schedule,
    // and the credit.
    await limited.click();
    const detail = page.getByRole('region', { name: 'Night details' });
    await expect(detail.getByText(/^Forecast \(Open-Meteo, fetched .+\): Limited/)).toBeVisible();
    await expect(detail.getByText('Clear: 30% cloud or less')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Weather data by Open-Meteo.com (CC BY 4.0)' })).toBeVisible();
    const clouds = detail.getByRole('group', { name: 'Cloud cover' });
    await expect(clouds).toBeVisible({ timeout: 30_000 });
    await expect(clouds.locator('.nights-cloud.is-cloudy').first()).toBeAttached();
    // The site's horizon file keeps the walled Heart out of a night with a
    // schedule. Only the next few nights have one, and the last cloudy one
    // may be beyond them.
    await cells.filter({ hasText: /\d h planned/ }).first().click();
    const plans = detail.getByRole('region', { name: /^This night:/ });
    await plans.getByText(/^Not this night: \d+ targets?$/).click();
    await expect(plans.getByRole('listitem').filter({ hasText: 'Heart behind the wall · IC 1805' })).toContainText('not above its horizon and altitude limits during darkness');
    await shot(page, 'nights-forecast', name => testInfo.outputPath(name));

    // Turned off, the site sends nothing and its nights are clear again.
    await openSettings(page);
    await selectSettingsSection(page, 'sites');
    await sites.getByRole('button', { name: `Edit ${siteName}` }).click();
    await card.getByLabel('Forecast', { exact: true }).selectOption('off');
    await card.getByRole('button', { name: 'Save site' }).click();
    await expect(card.getByText(`Saved ${siteName}.`)).toBeVisible();
    await expect(card.getByLabel('Forecast status')).toHaveCount(0);
    const after = await json(await request.post('/api/director/v1/night-plans', { data: { rigs: [rigId], nights: 3 } }));
    expect(after.rigs[0].forecast).toBeNull();
    expect(errors).toEqual([]);
  } finally {
    await request.delete(`/api/databases/${slug}`);
  }
});
