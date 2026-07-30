import { describe, expect, it } from 'vitest';
import {
  imageDetailNavigationState,
  imageDetailClosePath,
  imageDetailPath,
  imageDetailReturnView,
  sequenceReturnPositionFromState,
} from '../imageDetailRoutes';

describe('image detail routes', () => {
  it('marks details opened from Sequence and preserves their scope', () => {
    const params = new URLSearchParams('db=test&project=1&target=2&current=12');

    expect(imageDetailPath(12, params, 'sequence')).toBe(
      '/detail/12?db=test&project=1&target=2&current=12&returnTo=sequence'
    );
  });

  it('updates the Sequence cursor while navigating between Detail images', () => {
    const params = new URLSearchParams(
      'db=test&project=1&target=2&current=12&returnTo=sequence'
    );

    expect(imageDetailPath(13, params, 'sequence')).toBe(
      '/detail/13?db=test&project=1&target=2&current=13&returnTo=sequence'
    );
  });

  it('clears a stale Sequence origin when Images opens Detail', () => {
    const params = new URLSearchParams('db=test&project=1&returnTo=sequence');

    expect(imageDetailPath(12, params, 'grid')).toBe(
      '/detail/12?db=test&project=1'
    );
  });

  it('returns to Sequence without leaking the transient marker', () => {
    const params = new URLSearchParams(
      'db=test&project=1&target=2&current=12&returnTo=sequence'
    );

    expect(imageDetailReturnView(params)).toBe('sequence');
    expect(imageDetailClosePath(params)).toBe(
      '/sequence?db=test&project=1&target=2&current=12'
    );
  });

  it('uses Images as the safe fallback', () => {
    const params = new URLSearchParams('db=test&project=1&returnTo=unknown');

    expect(imageDetailReturnView(params)).toBe('grid');
    expect(imageDetailClosePath(params)).toBe('/grid?db=test&project=1');
  });

  it('validates Sequence scroll state', () => {
    const position = { scrollTop: 432, imageId: 12, offsetTop: 180 };
    expect(imageDetailNavigationState(position)).toEqual({ sequenceReturn: position });
    expect(imageDetailNavigationState({ ...position, scrollTop: -10 })).toEqual({
      sequenceReturn: { ...position, scrollTop: 0 },
    });
    expect(sequenceReturnPositionFromState({ sequenceReturn: position })).toEqual(position);
    expect(sequenceReturnPositionFromState({
      sequenceReturn: { ...position, offsetTop: Number.NaN },
    })).toBeNull();
    expect(sequenceReturnPositionFromState(null)).toBeNull();
  });
});
