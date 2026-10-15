import { describe, expect, it } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { useImageNavigation } from '../useImageNavigation';

const image = (id: number, acquired: number, filter: string) => ({
  id,
  project_id: 1,
  project_name: 'Bubble',
  project_display_name: 'Bubble',
  target_id: 42,
  target_name: 'Bubble Nebula',
  acquired_date: acquired,
  filter_name: filter,
  grading_status: 0,
  reject_reason: null,
  metadata: {},
  filesystem_path: null,
});

function wrapper(route: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

describe('detail next and previous', () => {
  const frames = [
    image(1, 100, 'HA'),
    image(2, 200, 'OIII'),
    image(3, 300, 'HA'),
    image(4, 400, 'OIII'),
  ];

  const navigation = (route: string, current: number) => {
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, error: null, status: 'ready', data: frames })),
    );
    return renderHook(() => ({
      ...useImageNavigation(current),
      path: useLocation().pathname,
    }), { wrapper: wrapper(route) });
  };

  it('walk the grid in capture order by default', async () => {
    const { result } = navigation('/detail/3?db=c925&project=1', 3);
    await waitFor(() => expect(result.current.allImages.map((entry) => entry.id)).toEqual([1, 3, 2, 4]));
    act(() => result.current.goToNext());
    await waitFor(() => expect(result.current.path).toBe('/detail/2'));
  });

  it('walk the grid\'s nights, split where the catalog\'s nights split', async () => {
    // Twenty frames from 19:30 JST on the 12th, every half hour. The
    // catalog splits at 03:30 UTC (12:30 JST); noon UTC would cut the night
    // at 21:00 JST and leave the evening's first three frames on the 11th.
    const evening = Date.UTC(2026, 9, 12, 10, 30) / 1000;
    const night = Array.from({ length: 20 }, (_, index) => image(100 + index, evening + index * 1800, 'HA'));
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, error: null, status: 'ready', data: night })),
      http.get('/api/db/:dbId/stats/overall', () =>
        HttpResponse.json({ success: true, error: null, data: { night_boundary_seconds: 3.5 * 3600 } })),
    );
    const { result } = renderHook(() => ({
      ...useImageNavigation(101),
      path: useLocation().pathname,
    }), { wrapper: wrapper('/detail/101?db=c925&project=1&grouping=date&expanded=2026-10-12') });
    await waitFor(() => expect(result.current.allImages).toHaveLength(20));
    expect(result.current.allImages.map((entry) => entry.id)).toEqual(night.map((entry) => entry.id));
    expect(result.current.canGoPrevious).toBe(true);
    act(() => result.current.goToPrevious());
    await waitFor(() => expect(result.current.path).toBe('/detail/100'));
  });

  it('walk it newest first when the grid shows it that way', async () => {
    const { result } = navigation('/detail/3?db=c925&project=1&order=newest', 3);
    await waitFor(() => expect(result.current.allImages.map((entry) => entry.id)).toEqual([3, 1, 4, 2]));
    expect(result.current.canGoPrevious).toBe(false);
    act(() => result.current.goToNext());
    await waitFor(() => expect(result.current.path).toBe('/detail/1'));
  });
});
