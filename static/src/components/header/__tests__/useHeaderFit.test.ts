import { describe, expect, it } from 'vitest';
import { needsStacking } from '../useHeaderFit';

const widths = { brand: 150, nav: 760, utilities: 380, gap: 12 };

describe('header fit', () => {
  it('keeps one row while the brand, views and utilities fit', () => {
    // 150 + 760 + 380 + 2 × 12 = 1314
    expect(needsStacking({ ...widths, available: 1314 }, false)).toBe(false);
    expect(needsStacking({ ...widths, available: 1313 }, false)).toBe(true);
  });

  it('needs a little room to spare before a stacked header goes back to one row', () => {
    expect(needsStacking({ ...widths, available: 1318 }, true)).toBe(true);
    expect(needsStacking({ ...widths, available: 1322 }, true)).toBe(false);
  });

  it('stacks when a long target name widens the views', () => {
    expect(needsStacking({ ...widths, nav: 1100, available: 1400 }, false)).toBe(true);
  });
});
