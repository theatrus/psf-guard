import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import {
  colorPreviewEnabled,
  setColorPreview,
  useColorPreview,
} from '../useColorPreview';

describe('useColorPreview', () => {
  beforeEach(() => {
    // The module reads storage once at import, so reset through the setter
    // rather than by clearing storage behind its back.
    setColorPreview(true);
    window.localStorage.clear();
  });

  it('starts on, because a colour camera should look like what it recorded', () => {
    const { result } = renderHook(() => useColorPreview());
    expect(result.current).toBe(true);
  });

  it('keeps every view in step', () => {
    // The grid, the overview, and the detail view render the same exposures.
    // If they held the preference separately, a frame would change appearance
    // on the way into the detail view.
    const grid = renderHook(() => useColorPreview());
    const detail = renderHook(() => useColorPreview());

    act(() => setColorPreview(false));

    expect(grid.result.current).toBe(false);
    expect(detail.result.current).toBe(false);
  });

  it('remembers the choice', () => {
    act(() => setColorPreview(false));
    expect(window.localStorage.getItem('psf-guard.color-preview')).toBe('false');
    expect(colorPreviewEnabled()).toBe(false);
  });

  it('stops listening once a view unmounts', () => {
    const { result, unmount } = renderHook(() => useColorPreview());
    unmount();
    // A leaked subscriber would keep setting state on an unmounted component.
    act(() => setColorPreview(false));
    expect(result.current).toBe(true);
  });

  it('survives storage being unavailable', () => {
    // Private browsing can refuse localStorage. The preference is a
    // convenience; refusing to render would not be.
    const original = window.localStorage.setItem;
    window.localStorage.setItem = () => {
      throw new Error('storage disabled');
    };
    try {
      expect(() => setColorPreview(false)).not.toThrow();
      expect(colorPreviewEnabled()).toBe(false);
    } finally {
      window.localStorage.setItem = original;
    }
  });
});
