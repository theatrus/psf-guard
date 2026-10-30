import type { OverlaySolution } from '@seiza/astro-overlay';
import { useHotkeys } from 'react-hotkeys-hook';
import { toggleMeasureOption, useMeasureOptions } from '../../hooks/useMeasureOptions';
import { usePixelGrid, usePixelProbe } from '../../hooks/usePixelProbe';
import { pixelAt, skyAt, useImagePointer, type Size, type ZoomView } from './imagePointer';

interface ViewerReadoutInput {
  container: React.RefObject<HTMLElement | null>;
  view: ZoomView;
  /** The bitmap on screen, as the zoom is calibrated against it. */
  bitmap: Size | null;
  /** The image's own grid when it is known before the file is read. */
  grid?: Size | null;
  /** The window route of the file behind the image; null when it has none. */
  windowPath: string | null;
  /** A plate solution of the image's grid. */
  solution?: OverlaySolution | null;
}

/**
 * The pointer readout's state for an image viewer: the shared Value and
 * RA/Dec choices with their M and R keys, the pixel under the pointer, its
 * samples and its sky position. Hand `track` and `leave` to the canvas and
 * `readout` to `PointerReadout`; `active` is true while either is on.
 */
export function useViewerReadout({
  container,
  view,
  bitmap,
  grid,
  windowPath,
  solution,
}: ViewerReadoutInput) {
  const options = useMeasureOptions();
  useHotkeys('m', () => toggleMeasureOption('value'), []);
  useHotkeys('r', () => toggleMeasureOption('sky'), []);
  const pointer = useImagePointer(container, view);
  const active = options.value || options.sky;
  const fileGrid = usePixelGrid(active ? windowPath : null);
  const imageGrid = fileGrid ?? grid ?? bitmap;
  const pixel = active && pointer.position && bitmap && imageGrid
    ? pixelAt(pointer.position, bitmap, imageGrid)
    : null;
  const probe = usePixelProbe(options.value ? windowPath : null, pixel);
  const sky = options.sky && pixel && imageGrid ? skyAt(solution, pixel, imageGrid) : null;
  return {
    options,
    active,
    track: pointer.track,
    leave: pointer.leave,
    readout: {
      options,
      pixel,
      anchor: pointer.anchor,
      probe: windowPath ? probe : null,
      sky,
    },
  };
}

/**
 * Why a stack's RA/Dec is not ready, from the query for its solution:
 * still loading, or failed. Undefined once the query has answered; the
 * inspector then explains a missing solution itself.
 */
export function stackSolutionNote(query: { isLoading: boolean; error: unknown }): string | undefined {
  if (query.isLoading) return 'Looking for the stack’s plate solution…';
  if (query.error) {
    return query.error instanceof Error ? query.error.message : 'The stack’s plate solution could not be loaded';
  }
  return undefined;
}
