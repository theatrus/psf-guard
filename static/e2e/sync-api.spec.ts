import {
  expect,
  request as playwrightRequest,
  test,
  type APIRequestContext,
} from '@playwright/test';
import * as crypto from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import {
  fitsLight,
  REVIEW_DB,
  REVIEW_TOKEN,
  TELESCOPE_DB,
  TELESCOPE_TOKEN,
} from './fixtures/sync';

/**
 * Two PSF Guard instances syncing with each other.
 *
 * Every other sync test stops short of this: the Rust suites drive the
 * handlers in-process, and the Settings specs mock the responses. Here two
 * separate `psf-guard server` processes, each with its own registry, cache,
 * and catalog, move a night's work between them over HTTP — and the result is
 * read back through the ordinary API, so a break anywhere between the token
 * check and the committed row fails these.
 *
 * PSF Guard ships no client for its own protocol; the plugin at the telescope
 * plays that part. These specs stand in for it, pulling a bundle from one
 * instance and posting it to the other, which is exactly the traffic a
 * coordinator generates.
 *
 * Neither grant comes from the Settings UI. Both instances are opened by
 * config-file blocks, and the receiving one deliberately runs without
 * `--allow-database-management`: accepting a sync must not require the grant
 * that lets a caller name server filesystem paths.
 */

const AS_TELESCOPE = { Authorization: `Bearer ${TELESCOPE_TOKEN}` };
const AS_REVIEW = { Authorization: `Bearer ${REVIEW_TOKEN}` };

/** The two sync instances, each addressed on its own port. */
let telescope: APIRequestContext;
let review: APIRequestContext;

test.beforeAll(async () => {
  telescope = await playwrightRequest.newContext({
    baseURL: process.env.PSF_GUARD_E2E_TELESCOPE_URL,
  });
  review = await playwrightRequest.newContext({
    baseURL: process.env.PSF_GUARD_E2E_REVIEW_URL,
  });
});

test.afterAll(async () => {
  await Promise.all([telescope.dispose(), review.dispose()]);
});

/** Pull a bundle off the telescope instance, as a coordinator would. */
async function exportFromTelescope(operation: string) {
  const response = await telescope.post('/api/sync/v1/exports', {
    headers: AS_TELESCOPE,
    data: {
      protocol_version: 1,
      catalog_id: TELESCOPE_DB,
      operation,
      reviewed_only: false,
    },
  });
  expect(response.status(), await response.text()).toBe(200);
  return (await response.json()).data.bundle;
}

/** Post it to the review instance and return the preview it holds. */
async function previewOnReview(
  operation: string,
  bundle: unknown
): Promise<string> {
  const response = await review.post('/api/sync/v1/previews', {
    headers: AS_REVIEW,
    data: {
      protocol_version: 1,
      catalog_id: REVIEW_DB,
      operation,
      bundle,
    },
  });
  expect(response.status(), await response.text()).toBe(200);
  return (await response.json()).data.preview_id;
}

/** Read the review instance's catalog the way its own UI would. */
async function reviewImages() {
  const response = await review.get(
    `/api/db/${REVIEW_DB}/images?project_id=10&target_id=10`
  );
  expect(response.status(), await response.text()).toBe(200);
  return (await response.json()).data as Array<{
    id: number;
    grading_status: number;
    reject_reason: string | null;
  }>;
}

test('each instance answers for its own catalog and no other', async () => {
  const near = await telescope.get('/api/sync/v1/capabilities', {
    headers: AS_TELESCOPE,
  });
  expect(near.status(), await near.text()).toBe(200);
  const capabilities = (await near.json()).data;
  expect(capabilities.protocol_version).toBe(1);
  expect(capabilities.capabilities).toEqual(
    expect.arrayContaining(['merge', 'push_grades', 'preview_refresh'])
  );
  expect(capabilities.catalogs).toHaveLength(1);
  expect(capabilities.catalogs[0].id).toBe(TELESCOPE_DB);

  // The review instance's key came from token_file, so this also proves the
  // server read the file and trimmed its trailing newline.
  const far = await review.get('/api/sync/v1/capabilities', {
    headers: AS_REVIEW,
  });
  expect(far.status(), await far.text()).toBe(200);
  expect((await far.json()).data.catalogs[0].id).toBe(REVIEW_DB);

  // Neither instance's key opens the other.
  expect(
    (await review.get('/api/sync/v1/capabilities', { headers: AS_TELESCOPE }))
      .status()
  ).toBe(403);
  expect(
    (await telescope.get('/api/sync/v1/capabilities', { headers: AS_REVIEW }))
      .status()
  ).toBe(403);
  expect((await telescope.get('/api/sync/v1/capabilities')).status()).toBe(403);
});

test('a merge moves a night from one instance to the other', async () => {
  const before = await reviewImages();
  expect(before, 'the review copy starts with one reviewed frame').toHaveLength(1);

  const bundle = await exportFromTelescope('merge');
  expect(bundle.payload_sha256).toHaveLength(64);
  const previewId = await previewOnReview('merge', bundle);

  // A preview writes nothing.
  expect(await reviewImages()).toHaveLength(1);

  const applied = await review.post(
    `/api/sync/v1/previews/${previewId}/apply`,
    { headers: AS_REVIEW }
  );
  expect(applied.status(), await applied.text()).toBe(200);
  expect((await applied.json()).data.state).toBe('applied');

  const after = await reviewImages();
  expect(after).toHaveLength(2);
  const rejected = after.filter((image) => image.grading_status === 2);
  expect(
    rejected,
    'the review done on this side must survive the merge'
  ).toHaveLength(1);
  expect(rejected[0].reject_reason).toBe('reviewed here');

  // One apply per preview.
  const again = await review.post(
    `/api/sync/v1/previews/${previewId}/apply`,
    { headers: AS_REVIEW }
  );
  expect(again.status()).toBe(404);
});

test('a destination that moved refuses the apply, then refreshes and applies', async () => {
  const bundle = await exportFromTelescope('merge');
  const previewId = await previewOnReview('merge', bundle);

  // Somebody regrades on the review instance between preview and apply,
  // through its ordinary UI route.
  const images = await reviewImages();
  const regraded = await review.put(
    `/api/db/${REVIEW_DB}/images/${images[0].id}/grade`,
    { data: { status: 'pending' } }
  );
  expect(regraded.status(), await regraded.text()).toBe(200);

  const stale = await review.post(`/api/sync/v1/previews/${previewId}/apply`, {
    headers: AS_REVIEW,
  });
  expect(stale.status()).toBe(409);

  // The refused apply kept the preview, so the coordinator re-reviews rather
  // than re-uploading its bundle.
  const kept = await review.get(`/api/sync/v1/previews/${previewId}`, {
    headers: AS_REVIEW,
  });
  expect(kept.status(), await kept.text()).toBe(200);

  const refreshed = await review.post(
    `/api/sync/v1/previews/${previewId}/refresh`,
    { headers: AS_REVIEW }
  );
  expect(refreshed.status(), await refreshed.text()).toBe(200);
  expect((await refreshed.json()).data.preview_id).toBe(previewId);

  const applied = await review.post(
    `/api/sync/v1/previews/${previewId}/apply`,
    { headers: AS_REVIEW }
  );
  expect(applied.status(), await applied.text()).toBe(200);
});

test('an export can be fetched again by ID', async () => {
  const created = await telescope.post('/api/sync/v1/exports', {
    headers: AS_TELESCOPE,
    data: {
      protocol_version: 1,
      catalog_id: TELESCOPE_DB,
      operation: 'push_grades',
      reviewed_only: true,
    },
  });
  expect(created.status(), await created.text()).toBe(200);
  const { export_id: exportId, bundle } = (await created.json()).data;

  const refetched = await telescope.get(`/api/sync/v1/exports/${exportId}`, {
    headers: AS_TELESCOPE,
  });
  expect(refetched.status(), await refetched.text()).toBe(200);
  expect((await refetched.json()).data.bundle).toEqual(bundle);

  const unknown = await telescope.get(
    '/api/sync/v1/exports/2b3c4d5e-0000-0000-0000-000000000000',
    { headers: AS_TELESCOPE }
  );
  expect(unknown.status()).toBe(404);
});

test('pixels travel by their own route, not inside the catalog bundle', async () => {
  // A catalog sync carries rows and any thumbnails stored in the database. It
  // does not carry acquisition files — those go through the image upload
  // grant, which the review instance holds separately.
  const filename = '2026-07-25_sync-nebula_Ha_300s.fits';
  const frame = fitsLight('Sync Nebula Core', '2026-07-25T22:14:00.000');
  const digest = crypto.createHash('sha256').update(frame).digest('hex');

  const uploaded = await review.post(`/api/db/${REVIEW_DB}/images/upload`, {
    headers: {
      ...AS_REVIEW,
      'x-content-sha256': digest,
      'x-psf-guard-database-id': REVIEW_DB,
    },
    multipart: {
      image: { name: filename, mimeType: 'application/fits', buffer: frame },
    },
  });
  expect(uploaded.status(), await uploaded.text()).toBe(200);
  const result = (await uploaded.json()).data;
  expect(result.sha256).toBe(digest);
  expect(result.bytes).toBe(frame.length);

  // The bytes landed where the config said they should.
  const landed = path.join(process.env.PSF_GUARD_E2E_SYNC_UPLOAD_DIR!, filename);
  expect(fs.existsSync(landed), `expected ${landed}`).toBe(true);
  expect(fs.readFileSync(landed).equals(frame)).toBe(true);

  // A key without the upload grant cannot ship frames, even to a catalog it
  // is otherwise allowed to sync.
  const refused = await review.post(`/api/db/${REVIEW_DB}/images/upload`, {
    headers: {
      Authorization: `Bearer ${TELESCOPE_TOKEN}`,
      'x-content-sha256': digest,
      'x-psf-guard-database-id': REVIEW_DB,
    },
    multipart: {
      image: { name: filename, mimeType: 'application/fits', buffer: frame },
    },
  });
  expect(refused.status()).toBe(403);
});

test('the receiving instance records what the remote client did', async () => {
  // The audit trail is the operator's only account of a remote write, so
  // prove a running server writes it rather than trusting the writer's own
  // unit test.
  const auditPath = path.join(
    process.env.PSF_GUARD_E2E_REVIEW_CACHE!,
    'remote-sync-audit.jsonl'
  );

  const bundle = await exportFromTelescope('push_planning');
  await previewOnReview('push_planning', bundle);
  await review.get('/api/sync/v1/capabilities', {
    headers: { Authorization: 'Bearer not-a-configured-sync-token-00' },
  });

  await expect
    .poll(
      () =>
        fs.existsSync(auditPath)
          ? fs
              .readFileSync(auditPath, 'utf8')
              .trim()
              .split('\n')
              .map((line) => JSON.parse(line))
          : [],
      { timeout: 5_000 }
    )
    .toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          action: 'preview',
          outcome: 'ok',
          operation: 'push_planning',
          catalog_id: REVIEW_DB,
          // The entry names the catalog the bundle came from, which is how an
          // operator ties a change back to the instance that sent it.
          source_id: TELESCOPE_DB,
        }),
        // A rejected key names no catalog but is still on the record.
        expect.objectContaining({
          action: 'capabilities',
          outcome: 'refused',
          catalog_id: '-',
        }),
      ])
    );
});
