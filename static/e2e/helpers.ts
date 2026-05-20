import type { APIRequestContext } from '@playwright/test';
import * as os from 'os';
import * as path from 'path';

/** Per-PID tmp directory; same value the playwright.config.ts and
 *  global-setup.ts agree on. */
export function tmpBase(): string {
  return (
    process.env.PSF_GUARD_E2E_TMP ??
    path.join(os.tmpdir(), `psf-guard-e2e-${process.pid}`)
  );
}

export function fixtureDbPath(): string {
  return process.env.PSF_GUARD_E2E_DB ?? path.join(tmpBase(), 'scheduler.sqlite');
}

export function fixtureImageDir(): string {
  return (
    process.env.PSF_GUARD_E2E_IMAGES ?? path.join(tmpBase(), 'images')
  );
}

/** Reset the server's database list to empty between specs that need a
 *  known-clean state. Uses the HTTP CRUD endpoints directly. */
export async function resetDatabases(request: APIRequestContext): Promise<void> {
  const res = await request.get('/api/databases');
  if (!res.ok()) {
    throw new Error(`GET /api/databases failed: ${res.status()}`);
  }
  const body = await res.json();
  const dbs: Array<{ id: string }> = body.data ?? [];
  for (const db of dbs) {
    const del = await request.delete(`/api/databases/${encodeURIComponent(db.id)}`);
    if (!del.ok()) {
      throw new Error(`DELETE /api/databases/${db.id} failed: ${del.status()}`);
    }
  }
}

/** Convenience: register the fixture DB so a spec can start "with one DB
 *  already configured." */
export async function registerFixtureDb(
  request: APIRequestContext,
  opts: { name?: string; slug?: string } = {}
): Promise<{ id: string; name: string }> {
  const res = await request.post('/api/databases', {
    data: {
      name: opts.name ?? 'Fixture Rig',
      db_path: fixtureDbPath(),
      image_dirs: [fixtureImageDir()],
      slug: opts.slug,
    },
  });
  if (!res.ok()) {
    const text = await res.text();
    throw new Error(`POST /api/databases failed: ${res.status()} ${text}`);
  }
  const body = await res.json();
  return body.data;
}

/** Poll the per-DB projects/overview endpoint until at least one project
 *  reports `has_files: true`. The cache-progress endpoint is racy on its
 *  own — it returns "not refreshing" both before the refresh starts AND
 *  after it finishes — so we look at the data the tests actually care
 *  about instead. This is a stronger guarantee than "refresh finished":
 *  it also confirms the directory-tree lookup actually matched our fixture
 *  files.
 */
export async function waitForCacheReady(
  request: APIRequestContext,
  dbId: string,
  opts: { timeoutMs?: number; pollMs?: number } = {}
): Promise<void> {
  const timeout = opts.timeoutMs ?? 30_000;
  const poll = opts.pollMs ?? 250;
  const deadline = Date.now() + timeout;

  while (Date.now() < deadline) {
    const res = await request.get(
      `/api/db/${encodeURIComponent(dbId)}/projects/overview`
    );
    if (res.ok()) {
      const body = await res.json();
      const projects: Array<{ has_files?: boolean }> = body.data ?? [];
      if (projects.some((p) => p.has_files)) return;
    }
    await new Promise((r) => setTimeout(r, poll));
  }
  throw new Error(
    `Cache refresh for db=${dbId} did not surface any project with has_files=true within ${timeout}ms`
  );
}
