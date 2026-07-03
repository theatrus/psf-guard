import { apiClient } from '../api/client';
import type { PreviewDescriptor } from '../api/types';

/**
 * Process-wide coordinator that batches "is this preview generated yet?" polls.
 *
 * The server generates missing preview/annotated PNGs asynchronously on a
 * bounded interactive queue and answers the image request with HTTP 202. Each
 * waiting `<img>` registers its descriptor here; a single timer coalesces all
 * pending descriptors (per database) into one POST to the batch
 * `generation-status` endpoint, so a grid of N generating images produces one
 * poll per tick, not N. When a descriptor becomes `ready` its subscribers are
 * told to reload; on `error` they fall back.
 */

export interface PollSubscriber {
  onReady: () => void;
  onError: (message?: string) => void;
}

interface Pending {
  descriptor: PreviewDescriptor;
  subscribers: Set<PollSubscriber>;
}

const POLL_INTERVAL_MS = 800;
// Cap per request so one huge grid doesn't send a giant body; extra descriptors
// simply poll on the next tick.
const MAX_BATCH = 100;

// dbId -> (descriptor key -> Pending)
const pendingByDb = new Map<string, Map<string, Pending>>();
let timer: ReturnType<typeof setInterval> | null = null;
let polling = false;

/** Test-only: clear all pending state and stop the timer, so the module
 *  singleton doesn't leak between unit tests. */
export function __resetForTest(): void {
  if (timer) {
    clearInterval(timer);
    timer = null;
  }
  pendingByDb.clear();
  polling = false;
}

export function descriptorKey(d: PreviewDescriptor): string {
  return [
    d.kind,
    d.imageId,
    d.size,
    d.stretch ?? '',
    d.midtone ?? '',
    d.shadow ?? '',
    d.maxStars ?? '',
  ].join('|');
}

/**
 * Register interest in a descriptor's readiness. Returns an unregister fn;
 * call it when the image loads, errors terminally, or the component unmounts.
 */
export function registerPending(
  dbId: string,
  descriptor: PreviewDescriptor,
  subscriber: PollSubscriber
): () => void {
  let byKey = pendingByDb.get(dbId);
  if (!byKey) {
    byKey = new Map();
    pendingByDb.set(dbId, byKey);
  }
  const key = descriptorKey(descriptor);
  let entry = byKey.get(key);
  if (!entry) {
    entry = { descriptor, subscribers: new Set() };
    byKey.set(key, entry);
  }
  entry.subscribers.add(subscriber);
  ensureTimer();

  return () => {
    const map = pendingByDb.get(dbId);
    const e = map?.get(key);
    if (!e) return;
    e.subscribers.delete(subscriber);
    if (e.subscribers.size === 0) map!.delete(key);
    if (map && map.size === 0) pendingByDb.delete(dbId);
  };
}

function ensureTimer() {
  if (timer) return;
  timer = setInterval(poll, POLL_INTERVAL_MS);
  // Kick one immediately so the first pending image doesn't wait a full tick.
  void poll();
}

function stopTimerIfIdle() {
  if (pendingByDb.size === 0 && timer) {
    clearInterval(timer);
    timer = null;
  }
}

function resolveEntry(
  dbId: string,
  key: string,
  notify: (subs: Set<PollSubscriber>) => void
) {
  const map = pendingByDb.get(dbId);
  const entry = map?.get(key);
  if (!entry) return;
  // Drop first so a subscriber's synchronous re-register (e.g. reload → new
  // pending on a fresh error) starts a clean entry.
  map!.delete(key);
  if (map!.size === 0) pendingByDb.delete(dbId);
  notify(entry.subscribers);
}

async function poll() {
  if (polling) return; // don't overlap slow ticks
  polling = true;
  try {
    for (const [dbId, byKey] of Array.from(pendingByDb.entries())) {
      const entries = Array.from(byKey.entries()); // [key, Pending][]
      for (let i = 0; i < entries.length; i += MAX_BATCH) {
        const chunk = entries.slice(i, i + MAX_BATCH);
        let statuses;
        try {
          statuses = await apiClient.getGenerationStatus(
            dbId,
            chunk.map(([, e]) => e.descriptor)
          );
        } catch {
          continue; // transient network error: retry next tick
        }
        chunk.forEach(([key], idx) => {
          const st = statuses[idx];
          if (!st) return;
          if (st.state === 'ready') {
            resolveEntry(dbId, key, (subs) => subs.forEach((s) => s.onReady()));
          } else if (st.state === 'error') {
            resolveEntry(dbId, key, (subs) =>
              subs.forEach((s) => s.onError(st.error))
            );
          }
          // 'generating' → keep polling
        });
      }
    }
  } finally {
    polling = false;
    stopTimerIfIdle();
  }
}

/**
 * Kick generation of an artifact and resolve when it's ready (or failed).
 * For the `new Image()` preload / zoom-switch paths that aren't rendered
 * `<img>` elements. Fetching the image URL returns 202 and enqueues; then we
 * ride the same batch poller. Resolves `true` when ready, `false` on error.
 */
export function ensurePreviewReady(
  dbId: string,
  imageUrl: string,
  descriptor: PreviewDescriptor
): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    let unregister: (() => void) | null = null;
    const finish = (ok: boolean) => {
      if (settled) return;
      settled = true;
      unregister?.();
      resolve(ok);
    };
    // Warm the cache: the 202 enqueues generation server-side. A 200 means it
    // was already cached — resolve immediately.
    const probe = new Image();
    probe.onload = () => finish(true);
    probe.onerror = () => {
      if (settled) return;
      unregister = registerPending(dbId, descriptor, {
        onReady: () => finish(true),
        onError: () => finish(false),
      });
    };
    probe.src = imageUrl;
  });
}
