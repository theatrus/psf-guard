import { useEffect, useMemo, useState } from 'react';
import { apiClient } from '../../api/client';
import type { DirectorCutoutRequest } from '../../api/directorTypes';

/** A 202 is polled quickly at first, since an offline map renders in well
 *  under a second, then settles to once a second for a slow provider. */
const IMAGE_POLL_STEPS_MS = [150, 250, 400, 600, 1000];
const IMAGE_POLL_LIMIT = 90;
const message = (error: unknown) => error instanceof Error ? error.message : 'Survey image request failed';

export function useDebounced<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => { const timer = setTimeout(() => setDebounced(value), delay); return () => clearTimeout(timer); }, [value, delay]);
  return debounced;
}

export interface LoadedCutout { url: string; key: string; request: DirectorCutoutRequest }

/** Images kept from earlier requests of the same survey, so the stage has a
 *  picture around and above the view before its own tile lands. */
export const CUTOUT_HISTORY = 8;

export interface SurveyCutout {
  /** The image on screen and the request it answers, so a caller can place
   *  it under a view that has moved on since. */
  image: LoadedCutout | null;
  /** The newest images of the current survey, oldest first; `image` is the last. */
  tiles: LoadedCutout[];
  status: 'idle' | 'loading' | 'ready' | 'failed';
  error: string;
  /** The image on screen is for an earlier request; the next one is on its way. */
  stale: boolean;
}

/** A survey image from the server's cutout cache. Keeps the last image up
 *  while the next one loads, polls a 202, and names a failure instead of
 *  hiding it. `null` asks for nothing. `prefetch` names images to fetch
 *  quietly once the main one is up (wider views of the same place, so a
 *  zoom out already has a picture); they join `tiles` as they land. */
export function useSurveyCutout(request: DirectorCutoutRequest | null, delayMs = 400, prefetch: DirectorCutoutRequest[] = []): SurveyCutout {
  // Debounce the request's content, not its identity: callers build the
  // object on every render, and a timer reset per render would never fire.
  const key = request ? JSON.stringify(request) : null;
  const debouncedKey = useDebounced(key, delayMs);
  const debounced = useMemo<DirectorCutoutRequest | null>(() => debouncedKey ? JSON.parse(debouncedKey) as DirectorCutoutRequest : null, [debouncedKey]);
  const [tiles, setTiles] = useState<LoadedCutout[]>([]);
  const image = tiles.length ? tiles[tiles.length - 1] : null;
  const [status, setStatus] = useState<SurveyCutout['status']>('idle');
  const [error, setError] = useState('');
  useEffect(() => {
    if (!debounced) return;
    const key = JSON.stringify(debounced);
    let cancelled = false;
    let attempts = 0;
    let timer: ReturnType<typeof setTimeout> | null = null;
    setStatus('loading'); setError('');
    const poll = async () => {
      if (cancelled) return;
      try {
        const result = await apiClient.fetchDirectorCutout(debounced);
        if (cancelled) return;
        if (result.state === 'ready') {
          const url = URL.createObjectURL(result.blob);
          setTiles(previous => {
            // A new survey starts a new set; within one, the oldest goes once the set is full.
            const kept = previous.filter(tile => tile.request.survey === debounced.survey && tile.key !== key);
            for (const gone of previous) if (!kept.includes(gone)) URL.revokeObjectURL(gone.url);
            const next = [...kept, { url, key, request: debounced }];
            while (next.length > CUTOUT_HISTORY) URL.revokeObjectURL(next.shift()!.url);
            return next;
          });
          setStatus('ready');
        } else if (result.state === 'generating') {
          if (++attempts >= IMAGE_POLL_LIMIT) { setStatus('failed'); setError('Survey image is taking too long; the last one stays up.'); return; }
          timer = setTimeout(poll, IMAGE_POLL_STEPS_MS[Math.min(attempts - 1, IMAGE_POLL_STEPS_MS.length - 1)]);
        } else { setStatus('failed'); setError(result.error); }
      } catch (cause) { if (!cancelled) { setStatus('failed'); setError(message(cause)); } }
    };
    void poll();
    // A view that moved on, or a page that went away, must not keep asking.
    return () => { cancelled = true; if (timer) clearTimeout(timer); };
  }, [debounced]);
  // Prefetches run one at a time after the main image, and only for images
  // not already held; a request that fails or is still rendering is left for
  // the next settled view to try again.
  const prefetchKey = JSON.stringify(prefetch);
  const held = tiles.map(tile => tile.key).join('|');
  useEffect(() => {
    if (status !== 'ready' || prefetch.length === 0) return;
    let cancelled = false;
    const wanted = prefetch.filter(item => !tiles.some(tile => tile.key === JSON.stringify(item)));
    const wait = (ms: number) => new Promise<void>(resolve => { timer = setTimeout(resolve, ms); });
    let timer: ReturnType<typeof setTimeout> | null = null;
    const run = async () => {
      for (const item of wanted) {
        // A wide view takes the server a moment to render; poll it like the main image, within reason.
        for (let attempt = 0; attempt < 40 && !cancelled; attempt += 1) {
          try {
            const result = await apiClient.fetchDirectorCutout(item);
            if (cancelled) return;
            if (result.state === 'generating') { await wait(IMAGE_POLL_STEPS_MS[Math.min(attempt, IMAGE_POLL_STEPS_MS.length - 1)]); continue; }
            if (result.state === 'ready') {
              const url = URL.createObjectURL(result.blob);
              const key = JSON.stringify(item);
              setTiles(previous => {
                if (previous.some(tile => tile.key === key) || (previous.length && previous[0].request.survey !== item.survey)) { URL.revokeObjectURL(url); return previous; }
                const next = [{ url, key, request: item }, ...previous];
                while (next.length > CUTOUT_HISTORY) URL.revokeObjectURL(next.splice(1, 1)[0].url);
                return next;
              });
            }
          } catch { /* a prefetch that fails is simply not there yet */ }
          break;
        }
      }
    };
    void run();
    return () => { cancelled = true; if (timer) clearTimeout(timer); };
  }, [status, prefetchKey, held]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => () => { setTiles(previous => { for (const tile of previous) URL.revokeObjectURL(tile.url); return []; }); }, []);
  return { image, tiles, status, error, stale: image !== null && image.key !== debouncedKey };
}
