import type { ReferenceRegion } from '../api/types';

export interface ImagePoint {
  x: number;
  y: number;
}

export const MIN_ARTIFACT_REGION_EDGE = 8;
export const MAX_ARTIFACT_REGION_EDGE = 512;

export function artifactRegionFromPoints(
  start: ImagePoint,
  end: ImagePoint,
  imageWidth: number,
  imageHeight: number
): ReferenceRegion | null {
  const left = Math.max(0, Math.floor(Math.min(start.x, end.x)));
  const top = Math.max(0, Math.floor(Math.min(start.y, end.y)));
  const right = Math.min(imageWidth, Math.ceil(Math.max(start.x, end.x)));
  const bottom = Math.min(imageHeight, Math.ceil(Math.max(start.y, end.y)));
  const width = right - left;
  const height = bottom - top;
  if (width < MIN_ARTIFACT_REGION_EDGE || height < MIN_ARTIFACT_REGION_EDGE) return null;
  if (width > MAX_ARTIFACT_REGION_EDGE || height > MAX_ARTIFACT_REGION_EDGE) return null;
  return { x: left, y: top, width, height };
}
