/**
 * Which end of the night Images and Sequence show first. Frames run in
 * capture order by default; newest first turns each group, each Sequence
 * strip and the session tabs around. See docs/SCREENING.md, "Order".
 */
export type ImageOrder = 'oldest' | 'newest';

/** The URL key Images and Sequence share: `order=newest`. The default,
 *  oldest first, leaves the URL clean. */
export const IMAGE_ORDER_PARAM = 'order';

export const DEFAULT_IMAGE_ORDER: ImageOrder = 'oldest';

export const IMAGE_ORDER_LABELS: Record<ImageOrder, string> = {
  oldest: 'Oldest first',
  newest: 'Newest first',
};

/** The order a URL value names, or null for none or one not known. */
export function parseImageOrder(raw: string | null | undefined): ImageOrder | null {
  return raw === 'newest' || raw === 'oldest' ? raw : null;
}

/** A list already in capture order, in the order chosen. */
export function inImageOrder<T>(chronological: readonly T[], order: ImageOrder): T[] {
  return order === 'newest' ? [...chronological].reverse() : [...chronological];
}
