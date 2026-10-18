import { expect, test } from '@playwright/test';
import { lockedCrateVersion } from './fixtures/astrometry';

test('partial data directory reports usable and missing Seiza resources', async ({
  request,
}) => {
  const response = await request.get('/api/astrometry/capabilities');
  expect(response.ok()).toBeTruthy();

  const body = await response.json();
  expect(body.success).toBe(true);
  expect(body.data).toMatchObject({
    seiza_version: lockedCrateVersion('seiza'),
    seiza_fits_version: lockedCrateVersion('seiza-fits'),
    features: {
      object_association: true,
      object_name_search: false,
      stellar_name_search: false,
      hinted_solve: true,
      blind_solve: false,
      transient_annotations: false,
      minor_body_annotations: false,
    },
    resources: {
      objects: {
        name: 'objects',
        status: 'available',
        format: 'SEIZAOB4',
      },
      stars: { status: 'available', format: 'SEIZAST2' },
      star_identifiers: { status: 'missing' },
      blind_index: { status: 'missing' },
      transients: { status: 'missing' },
      minor_bodies: { status: 'missing' },
    },
  });
  expect(body.data.resources.objects.path).toMatch(/objects\.bin$/);
  expect(body.data.resources.objects.size_bytes).toBeGreaterThan(0);
});

test('explicit validation opens and exhaustively validates the installed catalog', async ({
  request,
}) => {
  const response = await request.post('/api/astrometry/catalogs/validate');
  expect(response.ok()).toBeTruthy();

  const body = await response.json();
  expect(body.success).toBe(true);
  expect(body.data.all_configured_valid).toBe(false);

  const resources = body.data.resources as Array<{
    name: string;
    status: string;
    validated: boolean;
  }>;
  expect(resources.find((resource) => resource.name === 'objects')).toMatchObject({
    name: 'objects',
    status: 'available',
    validated: true,
  });
  // Gaia colors are optional: without them the catalog is not configured,
  // rather than missing like a required resource.
  expect(resources.find((resource) => resource.name === 'gaia_photometry')).toMatchObject({
    status: 'not_configured',
    validated: false,
  });
  expect(
    resources
      .filter((resource) => !['objects', 'stars', 'gaia_photometry'].includes(resource.name))
      .map((resource) => [resource.name, resource.status, resource.validated])
  ).toEqual([
    ['star_identifiers', 'missing', false],
    ['blind_index', 'missing', false],
    ['transients', 'missing', false],
    ['minor_bodies', 'missing', false],
  ]);
  expect(resources.find((resource) => resource.name === 'stars')).toMatchObject({
    name: 'stars',
    status: 'available',
    validated: true,
  });
});
