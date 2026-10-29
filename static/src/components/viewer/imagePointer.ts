import { useCallback, useState } from 'react';
import { pixelToWorld, type OverlaySolution } from '@seiza/astro-overlay';

export interface Size {
  width: number;
  height: number;
}

/** A pixel of the image's own grid, counted from 0 at the top left. */
export interface ImagePixel {
  x: number;
  y: number;
}

export interface ZoomView {
  scale: number;
  offsetX: number;
  offsetY: number;
}

/** A position on the displayed bitmap, in its pixels. */
export interface BitmapPosition {
  x: number;
  y: number;
}

/** The bitmap position under a client position, as the zoom places it. */
export function bitmapPositionUnder(
  clientX: number,
  clientY: number,
  container: DOMRect,
  view: ZoomView
): BitmapPosition | null {
  if (view.scale <= 0) return null;
  return {
    x: (clientX - container.left - view.offsetX) / view.scale,
    y: (clientY - container.top - view.offsetY) / view.scale,
  };
}

/**
 * The image pixel at a bitmap position. `image` is the grid the pixel is
 * counted on, which is larger when the bitmap is a reduced preview. Null off
 * the image.
 */
export function pixelAt(position: BitmapPosition, bitmap: Size, image: Size = bitmap): ImagePixel | null {
  if (bitmap.width <= 0 || bitmap.height <= 0) return null;
  const x = Math.floor(position.x * (image.width / bitmap.width));
  const y = Math.floor(position.y * (image.height / bitmap.height));
  if (x < 0 || y < 0 || x >= image.width || y >= image.height) return null;
  return { x, y };
}

/**
 * The sky position of a pixel's centre, in degrees, when `solution` has a
 * WCS. The solution may be on a grid of another size than the image's (a
 * frame solved at a different binning), so the pixel centre is carried
 * across in proportion.
 */
export function skyAt(
  solution: OverlaySolution | null | undefined,
  pixel: ImagePixel,
  image: Size
): [number, number] | null {
  if (!solution?.wcs || image.width <= 0 || image.height <= 0) return null;
  const x = (pixel.x + 0.5) * (solution.image_width / image.width) - 0.5;
  const y = (pixel.y + 0.5) * (solution.image_height / image.height) - 0.5;
  const [ra, dec] = pixelToWorld(solution as OverlaySolution & { wcs: NonNullable<OverlaySolution['wcs']> }, x, y);
  return Number.isFinite(ra) && Number.isFinite(dec) ? [ra, dec] : null;
}

/**
 * Follow the pointer over a zoomed image: `track` from the canvas's mouse
 * moves and wheel events, `leave` when it leaves. The position is on the
 * displayed bitmap; `pixelAt` counts it on the image's grid.
 */
export function useImagePointer(
  container: React.RefObject<HTMLElement | null>,
  view: ZoomView
) {
  const [position, setPosition] = useState<BitmapPosition | null>(null);
  const track = useCallback((event: { clientX: number; clientY: number }) => {
    const element = container.current;
    setPosition(element
      ? bitmapPositionUnder(event.clientX, event.clientY, element.getBoundingClientRect(), view)
      : null);
  }, [container, view]);
  const leave = useCallback(() => setPosition(null), []);
  return { position, track, leave };
}
