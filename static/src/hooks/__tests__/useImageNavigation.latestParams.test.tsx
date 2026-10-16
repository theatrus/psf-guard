import { describe, expect, it } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryRouter, RouterProvider, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { useImageNavigation } from '../useImageNavigation';
import { useUrlParams } from '../useUrlState';

/**
 * The detail's moves start from the URL as it stands
 * (useLatestSearchParams): a write made a moment before a step or a close,
 * such as a grade's selection or the cursor, goes with it rather than
 * being replaced by the address this render shows.
 */
const image = (id: number, acquired: number) => ({
  id, project_id: 1, project_name: 'Bubble', project_display_name: 'Bubble',
  target_id: 42, target_name: 'Bubble Nebula', acquired_date: acquired,
  filter_name: 'HA', grading_status: 0, reject_reason: null, metadata: {}, filesystem_path: null,
});

let navigation: ReturnType<typeof useImageNavigation> | null = null;
let write: ReturnType<typeof useUrlParams>['updateParams'] | null = null;

function Detail() {
  navigation = useImageNavigation(1);
  write = useUrlParams().updateParams;
  return <Where />;
}

function Where() {
  const { pathname, search } = useLocation();
  return <output data-testid="where">{pathname + search}</output>;
}

const where = () => screen.getByTestId('where').textContent ?? '';
const paramsOf = (url: string) => new URLSearchParams(url.split('?')[1] ?? '');

function mount() {
  server.use(
    http.get('/api/db/:dbId/images', () =>
      HttpResponse.json({ success: true, error: null, status: 'ready', data: [image(1, 100), image(2, 200)] })),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const router = createMemoryRouter(
    [
      { path: '/detail/:imageId', element: <Detail /> },
      { path: '/grid', element: <Where /> },
    ],
    { initialEntries: ['/detail/1?db=c925&project=1&current=1&returnTo=grid'] },
  );
  render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>);
}

describe('the detail moves from the URL as it stands', () => {
  it('steps with a selection written a moment before', async () => {
    mount();
    await waitFor(() => expect(navigation!.canGoNext).toBe(true));
    act(() => {
      write!({ selected: [1] });
      navigation!.goToNext();
    });
    await waitFor(() => expect(where()).toMatch(/^\/detail\/2\?/));
    expect(paramsOf(where()).get('selected')).toBe('1');
  });

  it('closes with the cursor written a moment before', async () => {
    mount();
    await waitFor(() => expect(navigation!.canGoNext).toBe(true));
    act(() => {
      write!({ current: 2 });
      navigation!.closeDetail();
    });
    await waitFor(() => expect(where()).toMatch(/^\/grid\?/));
    expect(paramsOf(where()).get('current')).toBe('2');
  });
});
