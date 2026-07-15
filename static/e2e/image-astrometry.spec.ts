import { expect, test } from '@playwright/test';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

let dbId: string;
let slugCounter = 0;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  slugCounter += 1;
  const entry = await registerFixtureDb(request, {
    name: `Astrometry Rig e2e ${slugCounter}`,
    slug: `astrometry-rig-e2e-${slugCounter}`,
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

test('associates catalog objects from real FITS headers without plate solving', async ({
  request,
}) => {
  const response = await request.get(
    `/api/db/${encodeURIComponent(dbId)}/images/1/astrometry`
  );
  expect(response.ok()).toBeTruthy();

  const body = await response.json();
  expect(body.success).toBe(true);
  expect(body.data).toMatchObject({
    image_id: 1,
    status: 'catalog_only',
    catalog_scope: 'estimated_field',
    hint_source: {
      source: 'fits_header',
    },
  });
  expect(body.data.solution).toBeUndefined();
  expect(body.data.catalog_hits).toEqual(
    expect.arrayContaining([
      expect.objectContaining({
        stable_id: 'openngc:NGC2632',
        source: 'OpenNGC',
        name: 'M 44',
        common_name: 'Beehive Cluster',
        kind: 'open-cluster',
      }),
    ])
  );
  expect(body.data.source_fingerprint.canonical_path).toMatch(/\.fit(s)?$/i);
});

test('renders and toggles the shared Seiza SVG overlay in image detail', async ({
  page,
}) => {
  await page.route('**/images/1/astrometry', async (route) => {
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          image_id: 1,
          status: 'solved',
          mode: 'embedded_wcs',
          hint_source: {
            ra_deg: 130.107,
            dec_deg: 19.66015,
            source: 'fits_wcs',
            header_keywords: ['CRVAL1', 'CRVAL2'],
          },
          solution: {
            center_ra_deg: 130.107,
            center_dec_deg: 19.66015,
            pixel_scale_arcsec_per_pixel: 0.629,
            matched_stars: 0,
            rms_arcsec: 0,
            image_width: 9576,
            image_height: 6388,
            wcs: {
              crval: [130.107, 19.66015],
              crpix: [4787.5, 3193.5],
              cd: [
                [-0.00017472, 0],
                [0, 0.00017472],
              ],
              ctype: ['RA---TAN', 'DEC--TAN'],
              cunit: ['deg', 'deg'],
            },
            footprint: [
              [130.99, 19.10],
              [129.22, 19.10],
              [129.21, 20.21],
              [131.0, 20.21],
            ],
            objects: [
              {
                stable_id: 'openngc:NGC2632',
                source: 'OpenNGC',
                aliases: ['NGC 2632', 'Praesepe'],
                parent_ids: [],
                alternate_ids: ['messier:M44'],
                alternate_sources: [],
                name: 'M 44',
                common_name: 'Beehive Cluster',
                kind: 'open-cluster',
                mag: 3.1,
                x: 4865,
                y: 3125,
                semi_major_px: 3108,
                semi_minor_px: 3108,
                angle_deg: 0,
                ra_deg: 130.0925,
                dec_deg: 19.67206,
                prominence: 0.99,
              },
            ],
            catalog_version: 'SEIZAOB3',
          },
          catalog_hits: [
            {
              stable_id: 'openngc:NGC2632',
              source: 'OpenNGC',
              aliases: ['NGC 2632', 'Praesepe'],
              parent_ids: [],
              alternate_ids: ['messier:M44'],
              alternate_sources: [],
              name: 'M 44',
              common_name: 'Beehive Cluster',
              kind: 'open-cluster',
              mag: 3.1,
              major_arcmin: 108.6,
              minor_arcmin: 108.6,
              position_angle_deg: null,
              ra_deg: 130.0925,
              dec_deg: 19.67206,
              center_inside: true,
              extent_only: false,
              distance_from_center_deg: 0.018,
              predicted_prominence: 0.99,
            },
          ],
          source_fingerprint: {
            canonical_path: '/fixtures/praesepe.fit',
            size_bytes: 1,
            modified_unix_seconds: 1,
          },
          computed_at: 1,
        },
      }),
    });
  });

  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

  const overlay = page.getByTestId('astrometry-overlay');
  await expect(page.getByText('Embedded FITS WCS')).toBeVisible();
  await expect(page.getByTestId('astrometry-panel').getByText('M 44')).toBeVisible();
  await expect(overlay).toBeVisible({ timeout: 30_000 });
  await expect(overlay).toHaveAttribute('data-overlay-version', '1');
  await expect(overlay.getByText('M 44')).toBeVisible();

  await page.getByRole('button', { name: /Sky overlay on/ }).click();
  await expect(overlay).toHaveCount(0);
  await page.getByRole('button', { name: /Show sky overlay/ }).click();
  await expect(page.getByTestId('astrometry-overlay')).toBeVisible();
});
