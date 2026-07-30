import { describe, expect, it } from 'vitest';
import { findGridNavigationIndex, type GridNavigationRect } from '../gridNavigation';

const items = [1, 2, 3, 4, 5];
const rects = new Map<number, GridNavigationRect>([
  [1, { top: 0, left: 0, width: 100 }],
  [2, { top: 0, left: 110, width: 100 }],
  [3, { top: 120, left: 0, width: 100 }],
  [4, { top: 120, left: 110, width: 100 }],
  [5, { top: 240, left: 0, width: 100 }],
]);

const navigate = (currentIndex: number, direction: 'next' | 'prev' | 'up' | 'down') =>
  findGridNavigationIndex(items, currentIndex, direction, item => rects.get(item) ?? null);

describe('findGridNavigationIndex', () => {
  it('moves left and right through item order', () => {
    expect(navigate(1, 'prev')).toBe(0);
    expect(navigate(1, 'next')).toBe(2);
  });

  it('uses the nearest column in the next or previous row', () => {
    expect(navigate(0, 'down')).toBe(2);
    expect(navigate(1, 'down')).toBe(3);
    expect(navigate(4, 'up')).toBe(2);
  });

  it('stays put when no item exists in that direction', () => {
    expect(navigate(0, 'up')).toBe(0);
    expect(navigate(4, 'down')).toBe(4);
  });
});
