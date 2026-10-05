import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { setSkyOverlay, skyOverlayEnabled, useSkyOverlay } from '../useSkyOverlay';

describe('useSkyOverlay', () => {
  beforeEach(() => {
    // The module reads storage once at import; reset through the setter.
    setSkyOverlay(false);
    window.localStorage.clear();
  });

  it('starts off: a solution on file is not a request to see it', () => {
    const { result } = renderHook(() => useSkyOverlay());
    expect(result.current).toBe(false);
  });

  it('remembers the last choice and shares it with every view', () => {
    const first = renderHook(() => useSkyOverlay());
    const second = renderHook(() => useSkyOverlay());
    act(() => setSkyOverlay(true));
    expect(first.result.current).toBe(true);
    expect(second.result.current).toBe(true);
    expect(window.localStorage.getItem('psf-guard.sky-overlay')).toBe('true');
    act(() => setSkyOverlay(false));
    expect(skyOverlayEnabled()).toBe(false);
    expect(window.localStorage.getItem('psf-guard.sky-overlay')).toBe('false');
  });

  it('survives storage being unavailable', () => {
    const setItem = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota');
    });
    const { result } = renderHook(() => useSkyOverlay());
    act(() => setSkyOverlay(true));
    expect(result.current).toBe(true);
    setItem.mockRestore();
  });
});
