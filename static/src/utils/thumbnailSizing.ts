export const THUMBNAIL_SIZE_MIN = 150;
export const THUMBNAIL_SIZE_MAX = 1200;
export const THUMBNAIL_SIZE_STEP = 50;

export function thumbnailGridColumns(size: number): string {
  return `repeat(auto-fill, minmax(min(${size}px, 100%), 1fr))`;
}
