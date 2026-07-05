import { expect, test } from '@playwright/test';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

/**
 * Regression coverage for async on-demand preview generation.
 *
 * On a cache miss the server no longer blocks the request generating the PNG;
 * it enqueues on a bounded interactive queue and returns HTTP 202. The
 * frontend loads optimistically, shows a "Generating…" indicator, and
 * *batch*-polls a single `generation-status` endpoint (one request for a whole
 * grid, not one poll per image) until the image is ready, then swaps it in.
 *
 * Each test registers the fixture DB under a UNIQUE slug, which gives it a
 * fresh per-DB cache directory — so the preview PNGs are guaranteed uncached
 * and actually exercise the generate → ready path (rather than a cache hit
 * left behind by an earlier spec).
 */

let dbId: string;
let slugCounter = 0;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  slugCounter += 1;
  const entry = await registerFixtureDb(request, {
    name: `Preview Gen e2e ${slugCounter}`,
    slug: `preview-gen-e2e-${slugCounter}`,
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

test('cache miss returns 202, and the batch status endpoint drives generation to ready', async ({
  request,
}) => {
  const base = `/api/db/${encodeURIComponent(dbId)}`;

  // Uncached (fresh slug): the preview GET must NOT block-and-200. It returns
  // 202 with a JSON "generating" body after enqueuing.
  const miss = await request.get(`${base}/images/1/preview?size=screen`);
  expect(
    miss.status(),
    `expected 202 on cache miss, body: ${await miss.text().catch(() => '<unread>')}`
  ).toBe(202);
  expect(miss.headers()['content-type']).toMatch(/application\/json/);
  expect((await miss.json()).data.state).toBe('generating');

  // Batch status for two images in ONE request → statuses parallel to input.
  const statusRes = await request.post(`${base}/images/generation-status`, {
    data: {
      requests: [
        { image_id: 1, kind: 'preview', size: 'screen' },
        { image_id: 3, kind: 'preview', size: 'screen' },
      ],
    },
  });
  expect(statusRes.ok()).toBeTruthy();
  const statuses = (await statusRes.json()).data.statuses as Array<{
    state: string;
  }>;
  expect(statuses).toHaveLength(2);
  for (const s of statuses) {
    expect(['generating', 'ready']).toContain(s.state);
  }

  // Poll the batch endpoint until image 1 finishes generating.
  await expect
    .poll(
      async () => {
        const r = await request.post(`${base}/images/generation-status`, {
          data: { requests: [{ image_id: 1, kind: 'preview', size: 'screen' }] },
        });
        return (await r.json()).data.statuses[0].state;
      },
      { timeout: 30_000, intervals: [500] }
    )
    .toBe('ready');

  // Now the same preview URL serves a real PNG (cache hit → 200).
  const hit = await request.get(`${base}/images/1/preview?size=screen`);
  expect(hit.status()).toBe(200);
  expect(hit.headers()['content-type']).toMatch(/^image\//);
  expect((await hit.body()).byteLength).toBeGreaterThan(0);
});

test('annotated images generate on-demand through the same queue', async ({
  request,
}) => {
  // Annotated generation is genuinely heavy: it runs full-frame star
  // detection (thousands of stars on a ~60 MP fixture, ~20 s single-threaded
  // on a fast dev box, more on a shared CI runner) before drawing overlays —
  // unlike the cheap preview stretch. `max_stars` does NOT bound that cost
  // (detection finds every star and only the *drawn* set is capped), so the
  // path is inherently slow. Give this one test a generous budget instead of
  // the 30 s default so a slow-but-correct runner doesn't flake.
  test.setTimeout(180_000);

  const base = `/api/db/${encodeURIComponent(dbId)}`;

  // Annotated cache miss also 202s (same async path as previews).
  const miss = await request.get(`${base}/images/1/annotated?size=large`);
  expect(miss.status()).toBe(202);
  expect(miss.headers()['content-type']).toMatch(/application\/json/);

  // Poll the batch endpoint with kind:'annotated' until it's ready. Fail fast
  // (with the server's message) if generation actually errors, rather than
  // silently burning the whole timeout on a job that will never be ready.
  const statusReq = {
    data: { requests: [{ image_id: 1, kind: 'annotated', size: 'large' }] },
  };
  const deadline = Date.now() + 150_000;
  let state = 'generating';
  while (Date.now() < deadline) {
    const r = await request.post(`${base}/images/generation-status`, statusReq);
    const status = (await r.json()).data.statuses[0];
    state = status.state;
    if (state === 'ready') break;
    if (state === 'error') {
      throw new Error(
        `annotated generation reported error: ${status.error ?? '<no detail>'}`
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  expect(state, 'annotated generation should reach ready before the deadline').toBe(
    'ready'
  );

  // The annotated endpoint now serves a PNG.
  const hit = await request.get(`${base}/images/1/annotated?size=large`);
  expect(hit.status()).toBe(200);
  expect(hit.headers()['content-type']).toMatch(/^image\//);
  expect((await hit.body()).byteLength).toBeGreaterThan(0);
});

test('grid shows a generating indicator, batch-polls status, and resolves to real pixels', async ({
  page,
}) => {
  // Observe the async-generation network behavior.
  const preview202: string[] = [];
  const batchPollSizes: number[] = [];

  page.on('response', (res) => {
    if (res.url().includes('/preview?') && res.status() === 202) {
      preview202.push(res.url());
    }
  });
  page.on('request', (req) => {
    if (
      req.method() === 'POST' &&
      req.url().includes('/images/generation-status')
    ) {
      let size = 0;
      try {
        size = (req.postDataJSON()?.requests ?? []).length;
      } catch {
        /* ignore non-JSON */
      }
      batchPollSizes.push(size);
    }
  });

  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  // While the queue works, the "Generating…" placeholder is shown.
  await expect(page.locator('.preview-status-box').first()).toBeVisible({
    timeout: 15_000,
  });

  // Every card's preview eventually decodes to real pixels (would be
  // naturalWidth 0 if the poll → reload path were broken).
  const cards = page.locator('.image-card');
  await expect(cards.first()).toBeVisible();
  const count = await cards.count();
  expect(count).toBeGreaterThan(1);
  for (let i = 0; i < count; i++) {
    const img = cards.nth(i).locator('img').first();
    await page.waitForFunction(
      (el) =>
        el instanceof HTMLImageElement && el.complete && el.naturalWidth > 0,
      await img.elementHandle(),
      { timeout: 40_000 }
    );
  }

  // Once every image is ready the indicator is gone.
  await expect(page.locator('.preview-status-box')).toHaveCount(0, {
    timeout: 10_000,
  });

  // Behavior: previews 202'd on miss (non-blocking) and the frontend polled
  // the single batch endpoint, coalescing multiple images into one request
  // rather than one status poll per image.
  expect(preview202.length, 'previews should 202 on a cache miss').toBeGreaterThan(
    0
  );
  expect(
    batchPollSizes.length,
    'frontend should poll the batch generation-status endpoint'
  ).toBeGreaterThan(0);
  expect(
    Math.max(0, ...batchPollSizes),
    'at least one status poll should coalesce multiple images'
  ).toBeGreaterThan(1);
});
