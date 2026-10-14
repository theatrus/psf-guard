import type { PropsWithChildren } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { afterEach, describe, expect, it } from 'vitest';
import { useGridState, useImageOrder } from '../useUrlState';
import { setDisplayPreferences, useDisplayPreferences } from '../useDisplayPreferences';

function Router({ children }: PropsWithChildren) {
  return <MemoryRouter initialEntries={['/grid']}>{children}</MemoryRouter>;
}

describe('useGridState', () => {
  it('supports a view-specific default thumbnail size', async () => {
    const { result } = renderHook(() => useGridState(150), { wrapper: Router });
    expect(result.current.imageSize).toBe(150);

    act(() => result.current.setImageSize(500));
    await waitFor(() => expect(result.current.imageSize).toBe(500));

    act(() => result.current.setImageSize(150));
    await waitFor(() => expect(result.current.imageSize).toBe(150));
  });

  it('keeps rapid functional multi-selection updates and a stable setter', async () => {
    const { result } = renderHook(() => useGridState(), { wrapper: Router });
    const initialSetter = result.current.setSelectedImages;

    act(() => {
      result.current.setCurrentImageSelection(11);
      result.current.setSelectedImages((selected) => new Set(selected).add(12));
    });

    await waitFor(() => {
      expect([...result.current.selectedImages]).toEqual([11, 12]);
      expect(result.current.currentImageId).toBe(11);
    });

    act(() => result.current.setCurrentImageId(12));
    await waitFor(() => expect(result.current.currentImageId).toBe(12));
    expect([...result.current.selectedImages]).toEqual([11, 12]);
    expect(result.current.setSelectedImages).toBe(initialSetter);
  });
});

describe('useImageOrder', () => {
  const preferences = {
    showNightChip: true,
    showAllChip: true, showCalibrationChip: true,
    advanceOnGrade: true,
    projectPickerGrouping: 'activity',
    libraryDensity: 'compact',
  } as const;
  afterEach(() => setDisplayPreferences({ ...preferences }));

  function at(route: string) {
    return function At({ children }: PropsWithChildren) {
      return <MemoryRouter initialEntries={[route]}>{children}</MemoryRouter>;
    };
  }
  const useOrderAndSearch = () => ({ ...useImageOrder(), search: useLocation().search });

  it('writes order=newest, leaves the default out, and remembers the choice', async () => {
    const { result } = renderHook(useOrderAndSearch, { wrapper: at('/grid?db=a&project=1') });
    expect(result.current.order).toBe('oldest');
    expect(result.current.search).not.toContain('order=');

    act(() => result.current.setOrder('newest'));
    await waitFor(() => expect(result.current.order).toBe('newest'));
    expect(new URLSearchParams(result.current.search).get('order')).toBe('newest');
    expect(new URLSearchParams(result.current.search).get('db')).toBe('a');
    expect(renderHook(() => useDisplayPreferences()).result.current.imageOrder).toBe('newest');

    act(() => result.current.setOrder('oldest'));
    await waitFor(() => expect(result.current.order).toBe('oldest'));
    expect(result.current.search).not.toContain('order=');
    expect(renderHook(() => useDisplayPreferences()).result.current.imageOrder).toBe('oldest');
  });

  it('reads a link\'s order first, and the remembered one when the link names none', () => {
    setDisplayPreferences({ ...preferences, imageOrder: 'newest' });
    expect(renderHook(useImageOrder, { wrapper: at('/sequence?db=a') }).result.current.order).toBe('newest');
    expect(renderHook(useImageOrder, { wrapper: at('/sequence?db=a&order=oldest') }).result.current.order).toBe('oldest');
    setDisplayPreferences({ ...preferences });
    expect(renderHook(useImageOrder, { wrapper: at('/grid?order=newest') }).result.current.order).toBe('newest');
    expect(renderHook(useImageOrder, { wrapper: at('/grid?order=sideways') }).result.current.order).toBe('oldest');
  });
});
