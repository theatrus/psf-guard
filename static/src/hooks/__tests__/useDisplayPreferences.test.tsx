import { beforeEach, describe, expect, it } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../useDisplayPreferences';

describe('display preferences', () => {
  beforeEach(() => {
    window.localStorage.removeItem('psf-guard.display-preferences');
    setDisplayPreferences({ showSecondaryScore: true });
  });

  it('defaults to showing the secondary score chip', () => {
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current.showSecondaryScore).toBe(true);
  });

  it('updates every subscriber when one toggle flips', () => {
    // Two hook instances stand in for the grid toolbar and the Sequence
    // toolbar: one shared preference, so flipping either flips both.
    const grid = renderHook(() => useDisplayPreferences());
    const sequence = renderHook(() => useDisplayPreferences());
    act(() => {
      setDisplayPreferences({ showSecondaryScore: false });
    });
    expect(grid.result.current.showSecondaryScore).toBe(false);
    expect(sequence.result.current.showSecondaryScore).toBe(false);
  });

  it('persists the choice to localStorage', () => {
    act(() => {
      setDisplayPreferences({ showSecondaryScore: false });
    });
    expect(
      JSON.parse(window.localStorage.getItem('psf-guard.display-preferences')!),
    ).toEqual({ showSecondaryScore: false });
  });

  it('falls back to the default on malformed stored values', () => {
    setDisplayPreferences({
      showSecondaryScore: 'yes' as unknown as boolean,
    });
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current.showSecondaryScore).toBe(true);
  });
});
