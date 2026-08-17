import { beforeEach, describe, expect, it } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../useDisplayPreferences';

describe('display preferences', () => {
  beforeEach(() => {
    window.localStorage.removeItem('psf-guard.display-preferences');
    setDisplayPreferences({ showNightChip: true, showAllChip: true });
  });

  it('defaults to showing both chip types', () => {
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current).toEqual({ showNightChip: true, showAllChip: true });
  });

  it('toggles each chip type independently', () => {
    act(() => {
      setDisplayPreferences({ showNightChip: false, showAllChip: true });
    });
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current.showNightChip).toBe(false);
    expect(result.current.showAllChip).toBe(true);
  });

  it('updates every subscriber when one toggle flips', () => {
    // Two hook instances stand in for the grid toolbar and the Sequence
    // toolbar: one shared preference, so flipping either flips both.
    const grid = renderHook(() => useDisplayPreferences());
    const sequence = renderHook(() => useDisplayPreferences());
    act(() => {
      setDisplayPreferences({ showNightChip: true, showAllChip: false });
    });
    expect(grid.result.current.showAllChip).toBe(false);
    expect(sequence.result.current.showAllChip).toBe(false);
  });

  it('persists the choice to localStorage', () => {
    act(() => {
      setDisplayPreferences({ showNightChip: false, showAllChip: false });
    });
    expect(
      JSON.parse(window.localStorage.getItem('psf-guard.display-preferences')!),
    ).toEqual({ showNightChip: false, showAllChip: false });
  });

  it('falls back to the default on malformed stored values', () => {
    setDisplayPreferences({
      showNightChip: 'yes' as unknown as boolean,
      showAllChip: true,
    });
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current.showNightChip).toBe(true);
  });

  it('honors the earlier single-switch stored form', () => {
    // A browser that stored the one-switch build's "off" keeps both chips
    // off after upgrading rather than silently turning them back on.
    window.localStorage.setItem(
      'psf-guard.display-preferences',
      JSON.stringify({ showSecondaryScore: false }),
    );
    window.dispatchEvent(
      new StorageEvent('storage', { key: 'psf-guard.display-preferences' }),
    );
    const { result } = renderHook(() => useDisplayPreferences());
    expect(result.current).toEqual({ showNightChip: false, showAllChip: false });
  });
});
