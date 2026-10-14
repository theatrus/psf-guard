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

  it('walk it newest first when the grid shows it that way', async () => {
    const { result } = navigation('/detail/3?db=c925&project=1&order=newest', 3);
    await waitFor(() => expect(result.current.allImages.map((entry) => entry.id)).toEqual([3, 1, 4, 2]));
    expect(result.current.canGoPrevious).toBe(false);
    act(() => result.current.goToNext());
    await waitFor(() => expect(result.current.path).toBe('/detail/1'));
  });
});
