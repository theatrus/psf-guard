import { expect, test } from '@playwright/test';

test('partial data directory reports usable and missing Seiza resources', async ({
  request,
}) => {
  const response = await request.get('/api/astrometry/capabilities');
  expect(response.ok()).toBeTruthy();

  const body = await response.json();
  expect(body.success).toBe(true);
  expect(body.data).toMatchObject({
    seiza_version: '0.8.0',
    seiza_fits_version: '0.1.6',
    features: {
      object_association: true,
      object_name_search: false,
      stellar_name_search: false,
      hinted_solve: false,
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
      stars: { status: 'missing' },
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
  expect(
    resources
      .filter((resource) => resource.name !== 'objects')
      .every(
        (resource) =>
          resource.status === 'missing' && resource.validated === false
      )
  ).toBe(true);
});
