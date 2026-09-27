import { useEffect, useMemo, useState } from 'react';
import { apiClient } from '../../api/client';
import type { DirectorCutoutRequest } from '../../api/directorTypes';

const IMAGE_POLL_MS = 1000;
const IMAGE_POLL_LIMIT = 90;
const message = (error: unknown) => error instanceof Error ? error.message : 'Survey image request failed';

export function useDebounced<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => { const timer = setTimeout(() => setDebounced(value), delay); return () => clearTimeout(timer); }, [value, delay]);
  return debounced;
}

export interface SurveyCutout {
  image: { url: string; key: string } | null;
  status: 'idle' | 'loading' | 'ready' | 'failed';
  error: string;
  /** The image on screen is for an earlier request; the next one is on its way. */
  stale: boolean;
}

/** A survey image from the server's cutout cache. Keeps the last image up
 *  while the next one loads, polls a 202, and names a failure instead of
 *  hiding it. `null` asks for nothing. */
export function useSurveyCutout(request: DirectorCutoutRequest | null, delayMs = 400): SurveyCutout {
  // Debounce the request's content, not its identity: callers build the
  // object on every render, and a timer reset per render would never fire.
  const key = request ? JSON.stringify(request) : null;
  const debouncedKey = useDebounced(key, delayMs);
  const debounced = useMemo<DirectorCutoutRequest | null>(() => debouncedKey ? JSON.parse(debouncedKey) as DirectorCutoutRequest : null, [debouncedKey]);
  const [image, setImage] = useState<{ url: string; key: string } | null>(null);
  const [status, setStatus] = useState<SurveyCutout['status']>('idle');
  const [error, setError] = useState('');
  useEffect(() => {
    if (!debounced) return;
    const key = JSON.stringify(debounced);
    let cancelled = false;
    let attempts = 0;
    setStatus('loading'); setError('');
    const poll = async () => {
      try {
        const result = await apiClient.fetchDirectorCutout(debounced);
        if (cancelled) return;
        if (result.state === 'ready') {
          const url = URL.createObjectURL(result.blob);
          setImage(previous => { if (previous) URL.revokeObjectURL(previous.url); return { url, key }; });
          setStatus('ready');
        } else if (result.state === 'generating') {
          if (++attempts >= IMAGE_POLL_LIMIT) { setStatus('failed'); setError('Survey image is taking too long; the last one stays up.'); return; }
          setTimeout(poll, IMAGE_POLL_MS);
        } else { setStatus('failed'); setError(result.error); }
      } catch (cause) { if (!cancelled) { setStatus('failed'); setError(message(cause)); } }
    };
    void poll();
    return () => { cancelled = true; };
  }, [debounced]);
  useEffect(() => () => { setImage(previous => { if (previous) URL.revokeObjectURL(previous.url); return null; }); }, []);
  return { image, status, error, stale: image !== null && image.key !== debouncedKey };
}
