export const THUMBNAIL_SIZE_MIN = 150;
export const THUMBNAIL_SIZE_MAX = 1200;
export const THUMBNAIL_SIZE_STEP = 50;

export function thumbnailGridColumns(size: number): string {
  return `repeat(auto-fill, minmax(min(${size}px, 100%), 1fr))`;
}

/** About how tall a grid card stands at this thumbnail size. An off-screen
 *  card holds this much room until it first renders. */
export function thumbnailCardHeightEstimate(size: number): number {
  return Math.max(256, Math.round(size * 0.75 + 75));
}
