import { expect, test } from '@playwright/test';

for (const width of [1440, 390]) {
  for (const condition of [
    { state: 'dewy', reason: 'dew_limit_reached', valid_for_ms: 30000, label: 'near dew point' },
    { state: 'unknown', reason: 'forecast_location_mismatch', valid_for_ms: 0, label: 'location mismatch' },
  ]) {
    test(`reported weather and forecast ${condition.label} stay separate at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 1000 });
      const now = Date.now();
      await page.route('**/api/director/v1/rigs/status', route => route.fulfill({ json: {
        success: true, error: null, status: 'ready', data: [{
          rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'Weather simulator', revision: 1 },
          catalog_slug: 'weather-test', catalog_name: 'Weather test', status: null,
          status_age_ms: null, status_stale: true, checkins: [], contacts: { program_pull: null, check_in: null, status: null },
          connectivity: { state: 'offline', last_contact_ms: null, age_ms: null }, assignments: [], pending_receipts: 0, recent_operations: [],
          weather: {
            rig_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', site_id: null, evaluated_at_ms: now,
            cloud_threshold_percent: 30, dew_limit_celsius: 2,
            reported: { rig_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', received_at_ms: now, sample: {
              sample_id: 'sample', profile_id: 'profile', source_name: 'NINA weather sensor', source_key: 'hash', reported_at_ms: now, connected: true,
              values: { cloud_percent: 85 },
            } },
            forecast: { provider: 'open_meteo', attribution: 'Weather data by Open-Meteo.com (CC BY 4.0)', fetched_at_ms: now, expires_at_ms: now + 3600000, fresh: true,
              hour: { start_ms: now, end_ms: now + 3600000, cloud_total: 10, temperature_celsius: 7, dew_point_celsius: 6, humidity_percent: 93 } },
            reported_cloud: { state: 'cloudy', reason: 'cloud_cover_above_threshold', valid_for_ms: 30000 },
            forecast_conditions: { state: condition.state, reason: condition.reason, valid_for_ms: condition.valid_for_ms },
          },
        }],
      } }));
      await page.goto('/#/rigs');
      // The empty scratch registry opens first-run settings after its async read.
      const settings = page.getByRole('heading', { name: 'PSF Guard Settings' });
      await expect(settings).toBeVisible();
      await page.getByRole('button', { name: 'Close settings' }).click();
      await expect(settings).not.toBeVisible();
      const live = page.getByRole('region', { name: 'Live rigs' });
      await expect(live).toContainText('NINA weather sensor · 85% cloud · cloudy');
      await expect(live).toContainText(`Open-Meteo · 10% cloud · ${condition.label}`);
      await expect(live.getByRole('link', { name: /CC BY 4.0/ })).toHaveAttribute('href', 'https://open-meteo.com/');
      await live.scrollIntoViewIfNeeded();
      await page.screenshot({ path: testInfo.outputPath(`weather-${width}.png`), fullPage: true });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    });
  }
}
