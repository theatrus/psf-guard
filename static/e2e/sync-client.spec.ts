import {
  expect,
  request as playwrightRequest,
  test,
  type APIRequestContext,
} from '@playwright/test';
import {
  REVIEW_DB,
  TELESCOPE_DB,
  TELESCOPE_TOKEN,
} from './fixtures/sync';

/**
 * PSF Guard syncing with PSF Guard, using its own client.
 *
 * `sync-api.spec.ts` drives the wire protocol directly, which proves the
 * server half. These prove the other half: the review instance configures the
 * telescope as a peer, then does the whole exchange itself — capabilities,
 * export, preview, apply — with the test only asking for the result and
 * reading it back through the ordinary API.
 *
 * The telescope runs without `--allow-database-management`. Accepting a sync
 * must never need the grant that lets a caller name server filesystem paths;
 * only the machine configuring a peer does, because storing one is registry
 * work.
 */

/** The instance someone sits at. It holds the peer list and drives the sync. */
let reviewer: APIRequestContext;

test.beforeAll(async () => {
  reviewer = await playwrightRequest.newContext({
    baseURL: process.env.PSF_GUARD_E2E_REVIEW_URL,
  });
});

test.afterAll(async () => {
  await reviewer.dispose();
});

/** Configure the telescope as a peer, or return the one already configured. */
async function ensurePeer(): Promise<string> {
  const existing = await reviewer.get('/api/peers');
  expect(existing.status(), await existing.text()).toBe(200);
  const peers = (await existing.json()).data as Array<{ id: string }>;
  if (peers.length > 0) {
    return peers[0].id;
  }
  const created = await reviewer.post('/api/peers', {
    data: {
      name: 'E2E telescope',
      base_url: process.env.PSF_GUARD_E2E_TELESCOPE_URL,
      token: TELESCOPE_TOKEN,
    },
  });
  expect(created.status(), await created.text()).toBe(200);
  return (await created.json()).data.id;
}

interface ImageRow {
  id: number;
  grading_status: number;
  reject_reason: string | null;
  metadata: { FileName?: string };
}

async function reviewImages(): Promise<ImageRow[]> {
  const response = await reviewer.get(
    `/api/db/${REVIEW_DB}/images?project_id=10&target_id=10`
  );
  expect(response.status(), await response.text()).toBe(200);
  return (await response.json()).data as ImageRow[];
}

/** Find a frame by the filename in its metadata, which is stable across specs
 *  in a way row IDs and grades are not — these specs share two catalogs and
 *  each leaves the previous one's writes behind. */
function byFile(images: ImageRow[], filename: string): ImageRow {
  const found = images.find((image) => image.metadata?.FileName === filename);
  expect(found, `no frame named ${filename} in ${JSON.stringify(images)}`).toBeTruthy();
  return found!;
}

test('a configured peer reports who it is and what it offers', async () => {
  const peerId = await ensurePeer();

  // The key was written once and never comes back.
  const listed = await reviewer.get('/api/peers');
  const peers = (await listed.json()).data as Array<Record<string, unknown>>;
  expect(peers[0].token_configured).toBe(true);
  expect(JSON.stringify(peers)).not.toContain(TELESCOPE_TOKEN);

  const checked = await reviewer.post(`/api/peers/${peerId}/check`);
  expect(checked.status(), await checked.text()).toBe(200);
  const check = (await checked.json()).data;
  expect(check.reachable).toBe(true);
  expect(check.product).toBe('PSF Guard');
  expect(check.protocol_version).toBe(1);
  expect(check.catalogs).toEqual([TELESCOPE_DB]);
  expect(check.capabilities).toContain('merge');
});

test('an unreachable peer is a state, not a failed request', async () => {
  const created = await reviewer.post('/api/peers', {
    data: {
      name: 'Nowhere',
      // A port nothing is listening on, so the client has to report a refusal
      // rather than hang or throw its way out of the handler.
      base_url: 'http://127.0.0.1:9',
      token: 'a-key-long-enough-to-be-accepted',
    },
  });
  expect(created.status(), await created.text()).toBe(200);
  const peerId = (await created.json()).data.id;

  const checked = await reviewer.post(`/api/peers/${peerId}/check`);
  expect(checked.status(), await checked.text()).toBe(200);
  const check = (await checked.json()).data;
  expect(check.reachable).toBe(false);
  expect(check.error).toBeTruthy();

  const removed = await reviewer.delete(`/api/peers/${peerId}`);
  expect(removed.status()).toBe(200);
});

test('this instance pulls a night off the telescope itself', async () => {
  const peerId = await ensurePeer();
  const before = await reviewImages();

  // A dry run writes nothing anywhere.
  const previewed = await reviewer.post(`/api/databases/${REVIEW_DB}/sync/remote`, {
    data: { peer_id: peerId, direction: 'pull', dry_run: true },
  });
  expect(previewed.status(), await previewed.text()).toBe(200);
  const preview = (await previewed.json()).data;
  expect(preview.applied).toBe(false);
  expect(preview.peer_catalog).toBe(TELESCOPE_DB);
  expect(await reviewImages()).toHaveLength(before.length);

  const applied = await reviewer.post(`/api/databases/${REVIEW_DB}/sync/remote`, {
    data: { peer_id: peerId, direction: 'pull', dry_run: false },
  });
  expect(applied.status(), await applied.text()).toBe(200);
  expect((await applied.json()).data.applied).toBe(true);

  // Both of the telescope's frames are now here, whatever was here before.
  const after = await reviewImages();
  byFile(after, 'sync-one.fits');
  byFile(after, 'sync-two.fits');

  // And a pull is idempotent: running it again changes nothing.
  const again = await reviewer.post(`/api/databases/${REVIEW_DB}/sync/remote`, {
    data: { peer_id: peerId, direction: 'pull', dry_run: true },
  });
  expect((await again.json()).data.summary.acquiredimage_inserted ?? 0).toBe(0);
});

test('this instance sends its grades back to the telescope', async () => {
  const peerId = await ensurePeer();
  // Both sides must know the frame before a grade can travel by GUID.
  await reviewer.post(`/api/databases/${REVIEW_DB}/sync/remote`, {
    data: { peer_id: peerId, direction: 'pull', dry_run: false },
  });

  const reason = `reviewed here at ${Date.now()}`;
  const frame = byFile(await reviewImages(), 'sync-two.fits');
  const regraded = await reviewer.put(
    `/api/db/${REVIEW_DB}/images/${frame.id}/grade`,
    { data: { status: 'rejected', reason } }
  );
  expect(regraded.status(), await regraded.text()).toBe(200);

  const pushed = await reviewer.post(`/api/databases/${REVIEW_DB}/sync/remote`, {
    data: {
      peer_id: peerId,
      direction: 'push_grades',
      dry_run: false,
      reviewed_only: true,
    },
  });
  expect(pushed.status(), await pushed.text()).toBe(200);
  const result = (await pushed.json()).data;
  expect(result.applied).toBe(true);

  // Read it back off the telescope, which knows nothing about this test.
  const telescope = await playwrightRequest.newContext({
    baseURL: process.env.PSF_GUARD_E2E_TELESCOPE_URL,
  });
  try {
    const response = await telescope.get(
      `/api/db/${TELESCOPE_DB}/images?project_id=1&target_id=1`
    );
    expect(response.status(), await response.text()).toBe(200);
    const remote = (await response.json()).data as ImageRow[];
    const landed = byFile(remote, 'sync-two.fits');
    expect(landed.grading_status).toBe(2);
    expect(landed.reject_reason).toBe(reason);
  } finally {
    await telescope.dispose();
  }
});

test('a sync names a direction it understands or refuses', async () => {
  const peerId = await ensurePeer();
  const response = await reviewer.post(`/api/databases/${REVIEW_DB}/sync/remote`, {
    data: { peer_id: peerId, direction: 'delete_everything', dry_run: true },
  });
  expect(response.status()).toBe(400);
  expect((await response.json()).error).toContain('delete_everything');
});
