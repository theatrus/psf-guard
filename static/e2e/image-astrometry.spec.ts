import { expect, test, type Page } from '@playwright/test';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

let dbId: string;
let slugCounter = 0;

async function renderedTransforms(page: Page) {
  const image = page.locator('.zoom-container img.detail-main-image').first();
  const overlay = page.getByTestId('astrometry-overlay');
  return {
    image: await image.evaluate((element) => getComputedStyle(element).transform),
    overlay: await overlay.evaluate((element) => getComputedStyle(element).transform),
  };
}

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

test('projects catalog objects from a real embedded FITS WCS without plate solving', async ({
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
    status: 'solved',
    mode: 'embedded_wcs',
    catalog_scope: 'embedded_footprint',
    hint_source: {
      source: 'fits_wcs',
    },
    solution: {
      image_width: 9576,
      image_height: 6388,
      wcs: {
        ctype: ['RA---TAN', 'DEC--TAN'],
        cunit: ['deg', 'deg'],
        radesys: 'ICRS',
      },
    },
    pointing: {
      target_in_frame: true,
    },
  });
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
  expect(body.data.solution.objects).toEqual(
    expect.arrayContaining([
      expect.objectContaining({
        stable_id: 'openngc:NGC2632',
        name: 'M 44',
        outlines: expect.arrayContaining([
          expect.objectContaining({
            geometry_id: 'openngc:NGC2632#e2e-outline',
            source_record_id: 'openngc:NGC2632',
            role: 'preferred-render',
            quality: 'curated',
            level: 'fixture-boundary',
          }),
        ]),
      }),
    ])
  );
  const m44 = body.data.solution.objects.find(
    (object: { stable_id: string }) => object.stable_id === 'openngc:NGC2632'
  );
  expect(m44.outlines[0].contours[0]).toMatchObject({ closed: true });
  expect(m44.outlines[0].contours[0].points).toHaveLength(4);
  expect(body.data.pointing.separation_arcsec).toBeLessThan(0.01);
  expect(body.data.source_fingerprint.canonical_path).toMatch(/\.fit(s)?$/i);
});

test('keeps catalog-only association distinct when a real FITS file has no WCS', async ({
  request,
}) => {
  const response = await request.get(
    `/api/db/${encodeURIComponent(dbId)}/images/2/astrometry`
  );
  expect(response.ok()).toBeTruthy();

  const body = await response.json();
  expect(body.data).toMatchObject({
    image_id: 2,
    status: 'catalog_only',
    catalog_scope: 'estimated_field',
    hint_source: { source: 'fits_header' },
  });
  expect(body.data.solution).toBeUndefined();
  expect(body.data.catalog_hits).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ stable_id: 'openngc:NGC2632', name: 'M 44' }),
    ])
  );

  const solveResponse = await request.post(
    `/api/db/${encodeURIComponent(dbId)}/images/2/astrometry`
  );
  expect(solveResponse.ok()).toBeTruthy();
  const solved = await solveResponse.json();
  expect(solved.data).toMatchObject({
    image_id: 2,
    status: 'solved',
    mode: 'hinted',
    catalog_scope: 'solved_footprint',
    solver_provenance: {
      seiza_version: '0.8.0',
      detection_backend: 'mtf_u8',
      star_catalog: { format: 'SEIZAST2' },
    },
    pointing: { target_in_frame: true },
  });
  expect(solved.data.solution.matched_stars).toBeGreaterThan(100);
  expect(solved.data.solution.rms_arcsec).toBeLessThan(3);
  expect(solved.data.solution.objects).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ stable_id: 'openngc:NGC2632', name: 'M 44' }),
    ])
  );

  const cachedResponse = await request.get(
    `/api/db/${encodeURIComponent(dbId)}/images/2/astrometry`
  );
  const cached = await cachedResponse.json();
  expect(cached.data).toMatchObject({ status: 'solved', mode: 'hinted' });
  expect(cached.data.solution.wcs.crval[0]).toBeCloseTo(
    solved.data.solution.wcs.crval[0],
    10
  );
  expect(cached.data.solution.matched_stars).toBe(
    solved.data.solution.matched_stars
  );
});

test('solves an ordinary acquisition frame on demand and enables the overlay', async ({
  page,
}) => {
  await page.goto(`/#/detail/2?db=${encodeURIComponent(dbId)}&project=1`);

  await expect(page.getByText('Expected field')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Solve field' })).toBeVisible();
  await page.keyboard.press('o');
  await expect(page.getByRole('button', { name: 'Solving field…' })).toBeDisabled();
  await expect(page.getByText('Hinted solve')).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId('astrometry-overlay')).toBeVisible();
  await expect(page.getByTestId('astrometry-overlay').getByText('M 44')).toBeVisible();

  await page.keyboard.press('o');
  await expect(page.getByTestId('astrometry-overlay')).toHaveCount(0);
  await page.keyboard.press('o');
  await expect(page.getByTestId('astrometry-overlay')).toBeVisible();
});

test('renders the real Seiza solution and keeps the overlay aligned while zooming and panning', async ({
  page,
}) => {

  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

  const overlay = page.getByTestId('astrometry-overlay');
  await expect(page.getByText('Embedded FITS WCS')).toBeVisible();
  await expect(page.getByTestId('astrometry-panel').getByText('M 44')).toBeVisible();
  await expect(overlay).toBeVisible({ timeout: 30_000 });
  await expect(overlay).toHaveAttribute('data-overlay-version', '1');
  await expect(overlay.getByText('M 44')).toBeVisible();
  const m44Outline = overlay.locator(
    '[data-stable-id="openngc:NGC2632"] .seiza-overlay__marker--outline'
  );
  await expect(m44Outline).toHaveAttribute(
    'data-geometry-id',
    'openngc:NGC2632#e2e-outline'
  );
  await expect(m44Outline).toHaveAttribute('data-outline-level', 'fixture-boundary');
  await expect(m44Outline).toHaveAttribute('stroke', '#f2ca72');
  await expect(m44Outline).toHaveAttribute('d', / Z$/);

  const initial = await renderedTransforms(page);
  expect(initial.overlay).toBe(initial.image);

  await page.getByTitle('100% (1)').click();
  await expect
    .poll(async () => {
      const current = await renderedTransforms(page);
      return current.overlay === current.image && current.image !== initial.image;
    })
    .toBe(true);

  const zoomed = await renderedTransforms(page);
  const container = page.locator('.zoom-container');
  const box = await container.boundingBox();
  expect(box).not.toBeNull();
  await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
  await page.mouse.down();
  await page.mouse.move(box!.x + box!.width / 2 + 80, box!.y + box!.height / 2 + 50, {
    steps: 5,
  });
  await page.mouse.up();
  await expect
    .poll(async () => {
      const current = await renderedTransforms(page);
      return current.overlay === current.image && current.image !== zoomed.image;
    })
    .toBe(true);

  await page.getByRole('button', { name: /Sky overlay on/ }).click();
  await expect(overlay).toHaveCount(0);
  await page.getByRole('button', { name: /Show sky overlay/ }).click();
  await expect(page.getByTestId('astrometry-overlay')).toBeVisible();
});
