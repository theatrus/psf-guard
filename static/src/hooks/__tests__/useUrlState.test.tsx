import type { PropsWithChildren } from 'react';
import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import { createMemoryRouter, MemoryRouter, RouterProvider, useLocation } from 'react-router-dom';
import { afterEach, describe, expect, it } from 'vitest';
import ThumbnailSizeControl from '../../components/ThumbnailSizeControl';
import { useGridState, useImageOrder } from '../useUrlState';
import { setDisplayPreferences, useDisplayPreferences } from '../useDisplayPreferences';

function Router({ children }: PropsWithChildren) {
  return <MemoryRouter initialEntries={['/grid']}>{children}</MemoryRouter>;
}

describe('useGridState', () => {
  it('supports a view-specific default thumbnail size', async () => {
    const { result } = renderHook(() => useGridState('sequence'), { wrapper: Router });
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

  it('shows each size step before the next key, so quick arrow keys all count', async () => {
    function SizeSlider() {
      const { imageSize, setImageSize } = useGridState();
      return <ThumbnailSizeControl id="grid-thumbnail-size" value={imageSize} onChange={setImageSize} />;
    }
    // The app's router: it applies a change as a transition unless told
    // to render it at once.
    const router = createMemoryRouter(
      [{ path: '/grid', element: <SizeSlider /> }],
      { initialEntries: ['/grid?size=300'] },
    );
    render(<RouterProvider router={router} />);
    const slider = await screen.findByRole<HTMLInputElement>('slider');
    expect(slider.value).toBe('300');

    // What a browser does for ArrowRight: step from the value the input
    // holds now, then report it. Outside act, so React renders when it
    // would in a browser, and the keys come before any later task.
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
    const arrowRight = async () => {
      setValue.call(slider, String(Number(slider.value) + 50));
      slider.dispatchEvent(new Event('input', { bubbles: true }));
      for (let tick = 0; tick < 10; tick++) await Promise.resolve();
    };
    const actEnvironment = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
    const previous = actEnvironment.IS_REACT_ACT_ENVIRONMENT;
    actEnvironment.IS_REACT_ACT_ENVIRONMENT = false;
    try {
      await arrowRight();
      expect(slider.value).toBe('350');
      await arrowRight();
      await arrowRight();
      expect(slider.value).toBe('450');
    } finally {
      actEnvironment.IS_REACT_ACT_ENVIRONMENT = previous;
    }
    await waitFor(() => expect(router.state.location.search).toBe('?size=450'));
    expect(slider.value).toBe('450');
  });
});

describe('saved View choices as defaults', () => {
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

  it('fill the grouping and size a link leaves out, and lose to one it names', () => {
    setDisplayPreferences({
      ...preferences,
      imageGrouping: 'session',
      imageGroupingAllProjects: 'project+date',
      gridThumbnailSize: 450,
      sequenceThumbnailSize: 200,
    });
    const grid = renderHook(() => useGridState(), { wrapper: at('/grid?db=a&project=1') }).result.current;
    expect(grid.groupingMode).toBe('session');
    expect(grid.imageSize).toBe(450);
    const sequence = renderHook(() => useGridState('sequence'), { wrapper: at('/sequence?db=a&project=1') }).result.current;
    expect(sequence.imageSize).toBe(200);
    // No project in the link is All projects, with its own grouping.
    expect(renderHook(() => useGridState(), { wrapper: at('/grid?db=a') }).result.current.groupingMode)
      .toBe('project+date');
    const linked = renderHook(() => useGridState(), { wrapper: at('/grid?db=a&project=1&grouping=date&size=600') }).result.current;
    expect(linked.groupingMode).toBe('date');
    expect(linked.imageSize).toBe(600);
  });

  it('are saved when chosen, each for its own scope and view', async () => {
    const { result } = renderHook(() => useGridState(), { wrapper: at('/grid?db=a&project=1') });
    act(() => result.current.setGroupingMode('both'));
    await waitFor(() => expect(result.current.groupingMode).toBe('both'));
    act(() => result.current.setGroupingMode('project+filter'));
    act(() => result.current.setImageSize(700));
    const saved = renderHook(() => useDisplayPreferences()).result.current;
    expect(saved.imageGrouping).toBe('both');
    expect(saved.imageGroupingAllProjects).toBe('project+filter');
    expect(saved.gridThumbnailSize).toBe(700);
    expect(saved.sequenceThumbnailSize).toBe(150);
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
