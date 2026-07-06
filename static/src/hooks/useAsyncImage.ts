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

interface AsyncImageRecord {
  baseSrc: string;
  state: AsyncImageState;
  reloadTick: number;
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
  const [record, setRecord] = useState<AsyncImageRecord>(() => ({
    baseSrc: src,
    state: 'loading',
    reloadTick: 0,
  }));
  const unregisterRef = useRef<null | (() => void)>(null);
  // Latest descriptor without making `onError` depend on its object identity.
  const descRef = useRef(descriptor);
  descRef.current = descriptor;

  // Reset when the underlying src changes (new image / size / stars toggle).
  useEffect(() => {
    setRecord((current) =>
      current.baseSrc === src
        ? current
        : { baseSrc: src, state: 'loading', reloadTick: 0 }
    );
    return () => {
      unregisterRef.current?.();
      unregisterRef.current = null;
    };
  }, [src]);

  const onLoad = useCallback(() => {
    unregisterRef.current?.();
    unregisterRef.current = null;
    setRecord((current) => ({
      baseSrc: src,
      state: 'ready',
      reloadTick: current.baseSrc === src ? current.reloadTick : 0,
    }));
  }, [src]);

  const onError = useCallback(() => {
    unregisterRef.current?.();
    if (!dbId) {
      unregisterRef.current = null;
      setRecord((current) => ({
        baseSrc: src,
        state: 'error',
        reloadTick: current.baseSrc === src ? current.reloadTick : 0,
      }));
      return;
    }
    setRecord((current) => ({
      baseSrc: src,
      state: 'generating',
      reloadTick: current.baseSrc === src ? current.reloadTick : 0,
    }));
    unregisterRef.current = registerPending(dbId, descRef.current, {
      onReady: () => {
        unregisterRef.current = null;
        setRecord((current) => ({
          baseSrc: src,
          state: 'loading',
          reloadTick: (current.baseSrc === src ? current.reloadTick : 0) + 1,
        })); // force <img> to re-fetch (now cached)
      },
      onError: () => {
        unregisterRef.current = null;
        setRecord((current) => ({
          baseSrc: src,
          state: 'error',
          reloadTick: current.baseSrc === src ? current.reloadTick : 0,
        }));
      },
    });
  }, [dbId, src]);

  const state = record.baseSrc === src ? record.state : 'loading';
  const reloadTick = record.baseSrc === src ? record.reloadTick : 0;
  const displaySrc =
    reloadTick > 0 ? withParam(src, 'v', String(reloadTick)) : src;
  return { src: displaySrc, state, onLoad, onError };
}

function withParam(url: string, key: string, value: string): string {
  const sep = url.includes('?') ? '&' : '?';
  return `${url}${sep}${key}=${encodeURIComponent(value)}`;
}
