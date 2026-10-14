import { describe, expect, it } from 'vitest';
import { inImageOrder, parseImageOrder } from '../imageOrder';

describe('image order', () => {
  it('reads the two orders from the URL and nothing else', () => {
    expect(parseImageOrder('newest')).toBe('newest');
    expect(parseImageOrder('oldest')).toBe('oldest');
    expect(parseImageOrder(null)).toBeNull();
    expect(parseImageOrder('Newest')).toBeNull();
    expect(parseImageOrder('')).toBeNull();
  });

  it('turns a list in capture order around for newest first, without touching it', () => {
    const frames = [1, 2, 3];
    expect(inImageOrder(frames, 'oldest')).toEqual([1, 2, 3]);
    expect(inImageOrder(frames, 'newest')).toEqual([3, 2, 1]);
    expect(frames).toEqual([1, 2, 3]);
  });
});
