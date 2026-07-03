import { useCallback, useEffect, useRef, useState } from 'react';
import { registerPending } from './previewPoll';
import type { PreviewDescriptor } from '../api/types';

export type AsyncImageState = 'loading' | 'ready' | 'generating' | 'error';

export interface AsyncImageResult {
  /** Feed this to `<img src>` (carries a cache-buster after a reload). */
  src: string;
  state: AsyncImageState;
  onLoad: () => void;
  onError: () => void;
}

/**
 * Optimistic image loading with batched poll-on-error.
 *
 * - Renders `src` directly, so a cache hit loads instantly with zero extra
 *   requests (the common case once pre-generation has run).
 * - On error — a 202 "generating" from the server, or a transient failure —
 *   it joins the shared batch poller (`previewPoll`) and reports
 *   `state: 'generating'`. When the artifact becomes ready it bumps a
 *   cache-buster and reloads (now a fast 200); a terminal failure reports
 *   `state: 'error'`.
 */
export function useAsyncImage(
  dbId: string | null | undefined,
  src: string,
  descriptor: PreviewDescriptor
): AsyncImageResult {
  const [state, setState] = useState<AsyncImageState>('loading');
  const [reloadTick, setReloadTick] = useState(0);
  const unregisterRef = useRef<null | (() => void)>(null);
  // Latest descriptor without making `onError` depend on its object identity.
  const descRef = useRef(descriptor);
  descRef.current = descriptor;

  // Reset when the underlying src changes (new image / size / stars toggle).
  useEffect(() => {
    setState('loading');
    setReloadTick(0);
    return () => {
      unregisterRef.current?.();
      unregisterRef.current = null;
    };
  }, [src]);

  const onLoad = useCallback(() => {
    unregisterRef.current?.();
    unregisterRef.current = null;
    setState('ready');
  }, []);

  const onError = useCallback(() => {
    unregisterRef.current?.();
    if (!dbId) {
      unregisterRef.current = null;
      setState('error');
      return;
    }
    setState('generating');
    unregisterRef.current = registerPending(dbId, descRef.current, {
      onReady: () => {
        unregisterRef.current = null;
        setState('loading');
        setReloadTick((t) => t + 1); // force <img> to re-fetch (now cached)
      },
      onError: () => {
        unregisterRef.current = null;
        setState('error');
      },
    });
  }, [dbId]);

  const displaySrc =
    reloadTick > 0 ? withParam(src, 'v', String(reloadTick)) : src;
  return { src: displaySrc, state, onLoad, onError };
}

function withParam(url: string, key: string, value: string): string {
  const sep = url.includes('?') ? '&' : '?';
  return `${url}${sep}${key}=${encodeURIComponent(value)}`;
}
