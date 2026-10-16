import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import GroupedImageGrid from '../GroupedImageGrid';
import { parseFlatGroupFilter } from '../../utils/flatGroupFilter';

const image = (id: number, filter = 'R') => ({
  id,
  project_id: 1,
  project_name: 'Bubble',
  project_display_name: 'Bubble',
  target_id: 42,
  target_name: 'Bubble Nebula',
  acquired_date: 1_705_352_400 + id,
  filter_name: filter,
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: `${id}.fits` },
  filesystem_path: `/images/${id}.fits`,
});

function wrapper(route: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>
          <Routes>
            <Route path="/grid" element={children} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    );
  };
}

const counts = { total: 2, own: 1, other_angle: 0, corrected: 0, undetermined: 1, no_same_side: 0 };

/** Lights 7, 8 and 10 (another rig's, another angle's) share filter R and
 *  the nights; only 7 and 8 take group g1's flats, and 8 is flagged. */
function serve(asked: string[] = [], computed = true) {
  server.use(
    http.get('/api/db/:dbId/images', () =>
      HttpResponse.json({ success: true, error: null, status: 'ready', data: [image(7), image(8), image(9, 'G'), image(10)] })),
    http.get('/api/db/:dbId/calibrations/flat-groups/use', ({ request }) => {
      asked.push(new URL(request.url).search);
      return HttpResponse.json({ success: true, error: null, status: 'ready', data: {
        computed,
        groups: computed ? [{
          key: 'g1', masters: [], lights: counts,
          projects: [{ project_id: 1, name: 'Bubble', lights: counts, image_ids: [7, 8], flagged_ids: [8] }],
        }] : [],
        lights: 3, pending: 0, unread: 0, refreshing: !computed,
      } });
    }),
  );
}

describe('the Images flat-group filter', () => {
  it('reads its value, and refuses a malformed one', () => {
    expect(parseFlatGroupFilter('c925:g1')).toEqual({ dbId: 'c925', key: 'g1', flagged: false });
    expect(parseFlatGroupFilter('c925:g1:flagged')).toEqual({ dbId: 'c925', key: 'g1', flagged: true });
    expect(parseFlatGroupFilter('c925')).toBeNull();
    expect(parseFlatGroupFilter('c925:g1:other')).toBeNull();
  });

  it('keeps exactly the lights the group counts', async () => {
    const asked: string[] = [];
    serve(asked);
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1') });
    await waitFor(() => expect(screen.getByText(/2 of 4 images/)).toBeInTheDocument());
    expect(document.querySelector('.grid-stats')).toHaveTextContent('Lights of one flat group');
    expect(asked).toEqual(['?group=g1']);
  });

  it('keeps only the flagged ones when asked', async () => {
    serve();
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1:flagged') });
    await waitFor(() => expect(screen.getByText(/1 of 4 images/)).toBeInTheDocument());
    expect(document.querySelector('.grid-stats')).toHaveTextContent('Flagged lights of one flat group');
  });

  it('hides nothing until the lights are known, and nothing for another database', async () => {
    serve([], false);
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1') });
    await waitFor(() => expect(screen.getByText(/4 of 4 images/)).toBeInTheDocument());
    expect(document.querySelector('.grid-stats')).toHaveTextContent('(updating)');
  });

  it('is not active for a value made in another database', async () => {
    const asked: string[] = [];
    serve(asked);
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=other:g1') });
    await waitFor(() => expect(screen.getByText(/4 of 4 images/)).toBeInTheDocument());
    expect(asked).toEqual([]);
  });
});
