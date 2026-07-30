export type GridNavigationDirection = 'next' | 'prev' | 'up' | 'down';

export interface GridNavigationRect {
  top: number;
  left: number;
  width: number;
}

export function findGridNavigationIndex<T>(
  items: readonly T[],
  currentIndex: number,
  direction: GridNavigationDirection,
  getRect: (item: T) => GridNavigationRect | null,
): number {
  if (currentIndex < 0 || currentIndex >= items.length) return currentIndex;
  if (direction === 'next') return Math.min(currentIndex + 1, items.length - 1);
  if (direction === 'prev') return Math.max(currentIndex - 1, 0);

  const currentRect = getRect(items[currentIndex]);
  if (!currentRect) return currentIndex;

  const currentCenter = currentRect.left + currentRect.width / 2;
  const sign = direction === 'down' ? 1 : -1;
  let result = currentIndex;
  let bestVerticalDistance = Number.POSITIVE_INFINITY;
  let bestHorizontalDistance = Number.POSITIVE_INFINITY;

  items.forEach((item, index) => {
    if (index === currentIndex) return;
    const rect = getRect(item);
    if (!rect) return;

    const verticalDistance = (rect.top - currentRect.top) * sign;
    if (verticalDistance <= 4) return;
    const horizontalDistance = Math.abs((rect.left + rect.width / 2) - currentCenter);
    const isCloserRow = verticalDistance < bestVerticalDistance - 4;
    const isCloserColumn = Math.abs(verticalDistance - bestVerticalDistance) <= 4
      && horizontalDistance < bestHorizontalDistance;
    if (isCloserRow || isCloserColumn) {
      result = index;
      bestVerticalDistance = verticalDistance;
      bestHorizontalDistance = horizontalDistance;
    }
  });

  return result;
}
