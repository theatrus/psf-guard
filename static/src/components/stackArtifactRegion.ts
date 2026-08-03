import type { ReferenceRegion } from '../api/types';

export interface ImagePoint {
  x: number;
  y: number;
}

export const MIN_ARTIFACT_REGION_EDGE = 8;
export const MAX_ARTIFACT_REGION_EDGE = 512;

interface Span {
  origin: number;
  length: number;
}

/**
 * One axis of the selection. The corner the drag began at stays put and the
 * moving edge stops at the widest region the search accepts, so dragging past
 * that grows nothing instead of voiding the box.
 */
function clampedSpan(anchor: number, moving: number, extent: number): Span {
  const from = Math.min(Math.max(anchor, 0), extent);
  const to = Math.min(Math.max(moving, 0), extent);
  // Both edges round outward, so a box never reads as smaller than it looks.
  if (to >= from) {
    const origin = Math.floor(from);
    const length = Math.min(Math.ceil(to) - origin, MAX_ARTIFACT_REGION_EDGE);
    return { origin, length };
  }
  const end = Math.ceil(from);
  const length = Math.min(end - Math.floor(to), MAX_ARTIFACT_REGION_EDGE);
  return { origin: end - length, length };
}

/**
 * The region a drag selects, in image pixels, or null while it is still too
 * small to search. Oversized drags are clamped rather than rejected: the box
 * stops growing at the limit and stays selectable.
 */
export function artifactRegionFromPoints(
  start: ImagePoint,
  end: ImagePoint,
  imageWidth: number,
  imageHeight: number
): ReferenceRegion | null {
  const horizontal = clampedSpan(start.x, end.x, imageWidth);
  const vertical = clampedSpan(start.y, end.y, imageHeight);
  if (
    horizontal.length < MIN_ARTIFACT_REGION_EDGE ||
    vertical.length < MIN_ARTIFACT_REGION_EDGE
  ) {
    return null;
  }
  return {
    x: horizontal.origin,
    y: vertical.origin,
    width: horizontal.length,
    height: vertical.length,
  };
}

/** Whether a region has grown to the limit on either side. */
export function isArtifactRegionCapped(region: ReferenceRegion): boolean {
  return (
    region.width >= MAX_ARTIFACT_REGION_EDGE || region.height >= MAX_ARTIFACT_REGION_EDGE
  );
}
