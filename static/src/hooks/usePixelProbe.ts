import { useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import {
  decodePixelWindow,
  sampleAt,
  tileFor,
  type DecodedWindow,
} from '../utils/pixelWindow';

/** What the readout knows about the samples under the pointer. */
export type PixelProbe =
  | { state: 'off' }
  | { state: 'loading' }
  | { state: 'ready'; values: number[] }
  | { state: 'error'; message: string };

/** Tiles kept per image: a few screens of pointer travel at 1:1. */
const MAX_TILES = 64;
/** How long the pointer rests on a new tile before it is read. */
const READ_DELAY_MS = 40;

/** The tiles read from one image, most recently used last. */
class TileCache {
  private ready = new Map<string, DecodedWindow>();
  private failed = new Map<string, string>();
  private pending = new Set<string>();

  lookup(key: string): DecodedWindow | string | undefined {
    const tile = this.ready.get(key);
    if (tile) {
      this.ready.delete(key);
      this.ready.set(key, tile);
      return tile;
    }
    return this.failed.get(key);
  }

  read(path: string, key: string, x: number, y: number, done: () => void): void {
    if (this.ready.has(key) || this.failed.has(key) || this.pending.has(key)) return;
    this.pending.add(key);
    apiClient
      .getPixelWindow(path, tileFor(x, y))
      .then((window) => {
        this.ready.set(key, decodePixelWindow(window));
        while (this.ready.size > MAX_TILES) {
          const oldest = this.ready.keys().next().value;
          if (oldest === undefined) break;
          this.ready.delete(oldest);
        }
      })
      .catch((error: unknown) => {
        this.failed.set(key, error instanceof Error ? error.message : 'Pixel values could not be read');
      })
      .finally(() => {
        this.pending.delete(key);
        done();
      });
  }
}

/**
 * The linear samples under a pixel, one per plane, read a tile at a time
 * from the image file behind `path` (a window route; see
 * `apiClient.fitsWindowPath`). `null` for either argument turns it off.
 */
export function usePixelProbe(
  path: string | null,
  point: { x: number; y: number } | null
): PixelProbe {
  const cache = useMemo(() => (path ? new TileCache() : null), [path]);
  const [, setArrivals] = useState(0);
  const tile = point ? tileFor(point.x, point.y) : null;
  const key = tile ? `${tile.x},${tile.y}` : null;
  const found = cache && key ? cache.lookup(key) : undefined;

  useEffect(() => {
    if (!cache || !path || !tile || !key || found !== undefined) return;
    const timer = window.setTimeout(() => {
      cache.read(path, key, tile.x, tile.y, () => setArrivals((count) => count + 1));
    }, READ_DELAY_MS);
    return () => window.clearTimeout(timer);
    // The tile's key stands for its origin.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cache, path, key, found]);

  if (!cache || !point) return { state: 'off' };
  if (found === undefined) return { state: 'loading' };
  if (typeof found === 'string') return { state: 'error', message: found };
  const values = sampleAt(found, point.x, point.y);
  return values ? { state: 'ready', values } : { state: 'off' };
}

/**
 * The size of the file behind a window route, which the pointer is counted
 * on. A viewer's bitmap can be a reduced preview, and a copy of a frame can
 * be another size than the frame. One one-pixel read per file.
 */
export function usePixelGrid(path: string | null): { width: number; height: number } | null {
  const { data } = useQuery({
    queryKey: ['pixel-grid', path],
    queryFn: () => apiClient.getPixelWindow(path!, { x: 0, y: 0, width: 1, height: 1 }),
    enabled: !!path,
    staleTime: Infinity,
    retry: false,
  });
  return data ? { width: data.image_width, height: data.image_height } : null;
}
