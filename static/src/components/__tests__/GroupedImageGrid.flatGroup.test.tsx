import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import GroupedImageGrid from '../GroupedImageGrid';
import { parseFlatGroupFilter } from '../../utils/flatGroupFilter';
import { imageFilterCount } from '../../utils/imageFilterCount';

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

function Where() {
  const location = useLocation();
  return <output data-testid="where">{location.search}</output>;
}

function wrapper(route: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>
          <Routes>
            <Route path="/grid" element={<>{children}<Where /></>} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    );
  };
}

const where = () => new URLSearchParams(screen.getByTestId('where').textContent ?? '');
const stats = () => document.querySelector('.grid-stats');
const ok = (data: unknown) => HttpResponse.json({ success: true, error: null, status: 'ready', data });
const counts = { total: 2, own: 1, other_angle: 0, corrected: 0, undetermined: 1, no_same_side: 0 };
const images = () => http.get('/api/db/:dbId/images', () => ok([image(7), image(8), image(9, 'G'), image(10)]));

/** Lights 7, 8 and 10 share filter R and the nights; only 7 and 8 take
 *  group g1's flats, and 8 is flagged. */
function serve(asked: string[] = [], computed = true) {
  server.use(
    images(),
    http.get('/api/db/:dbId/calibrations/flat-groups/use', ({ request }) => {
      asked.push(new URL(request.url).search);
      return ok({
        computed,
        groups: computed ? [{
          key: 'g1', masters: [], lights: counts,
          projects: [{ project_id: 1, name: 'Bubble', lights: counts, image_ids: [7, 8], flagged_ids: [8] }],
        }] : [],
        lights: 3, pending: 0, unread: 0, refreshing: !computed,
        ...(computed ? { group_found: true, group_key: 'g1' } : {}),
      });
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

  it('counts as a filter only in the database it was made in', () => {
    const filters = { status: 'all', filterNames: [], dateRange: { start: null, end: null }, searchTerm: '', flatGroup: 'c925:g1' };
    expect(imageFilterCount(filters, 'c925')).toBe(1);
    expect(imageFilterCount(filters, 'other')).toBe(0);
    expect(imageFilterCount(filters)).toBe(0);
  });

  it('keeps exactly the lights the group counts', async () => {
    const asked: string[] = [];
    serve(asked);
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1') });
    await waitFor(() => expect(screen.getByText(/2 of 4 images/)).toBeInTheDocument());
    expect(stats()).toHaveTextContent('Lights of one flat group');
    expect(asked).toEqual(['?group=g1']);
    expect(screen.getByRole('group', { name: 'Scope: Lights of a flat group' })).toBeInTheDocument();
  });

  it('keeps only the flagged ones when asked', async () => {
    serve();
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1:flagged') });
    await waitFor(() => expect(screen.getByText(/1 of 4 images/)).toBeInTheDocument());
    expect(stats()).toHaveTextContent('Flagged lights of one flat group');
  });

  it('shows no light, so no bulk action reaches one, until the lights are known', async () => {
    serve([], false);
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1') });
    await waitFor(() => expect(screen.getByText(/0 of 4 images/)).toBeInTheDocument());
    expect(screen.getByTestId('flat-group-filter-state')).toHaveTextContent("Finding the group's lights…");
    expect(stats()).toHaveTextContent('finding them');
    expect(document.querySelectorAll('[data-image-id]')).toHaveLength(0);
  });

  it('is not active for a value made in another database', async () => {
    const asked: string[] = [];
    serve(asked);
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=other:g1') });
    await waitFor(() => expect(screen.getByText(/4 of 4 images/)).toBeInTheDocument());
    expect(asked).toEqual([]);
    expect(screen.queryByRole('group', { name: /^Scope: / })).toBeNull();
  });

  it('comes off with its chip, and with Clear', async () => {
    serve();
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1&filter=R') });
    await waitFor(() => expect(screen.getByText(/2 of 4 images/)).toBeInTheDocument());
    // The grid has put its cursor on the first light.
    await waitFor(() => expect(where().get('current')).toBe('7'));
    fireEvent.click(screen.getByRole('button', { name: "Show every light, not only the flat group's" }));
    await waitFor(() => expect(where().get('flatGroup')).toBeNull());
    expect(where().get('filter')).toBe('R');
  });

  it('comes off with Clear, with the other filters', async () => {
    serve();
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1&filter=R') });
    await waitFor(() => expect(screen.getByText(/2 of 4 images/)).toBeInTheDocument());
    // The grid has put its cursor on the first light.
    await waitFor(() => expect(where().get('current')).toBe('7'));
    fireEvent.click(screen.getByRole('button', { name: /^Clear/ }));
    await waitFor(() => expect(where().get('flatGroup')).toBeNull());
    expect(where().get('filter')).toBeNull();
    await waitFor(() => expect(screen.getByText(/4 of 4 images/)).toBeInTheDocument());
  });

  // A group merged into an older one after a regroup: its key went. The
  // grid says so, rather than showing an empty grid as if it fed no light.
  it('says a group that no longer exists is gone, and offers every light', async () => {
    server.use(
      images(),
      http.get('/api/db/:dbId/calibrations/flat-groups/use', () => ok({
        computed: true,
        groups: [{ key: 's-older', masters: [], lights: counts,
          projects: [{ project_id: 1, name: 'Bubble', lights: counts }] }],
        lights: 4, pending: 0, unread: 0, refreshing: false, group_found: false,
      })),
    );
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:s-merged') });
    await waitFor(() => expect(screen.getByText(/of 4 images/)).toBeInTheDocument());
    await waitFor(() => expect(stats()).toHaveTextContent('This flat group no longer exists'));
    expect(screen.getByTestId('flat-group-filter-state')).toHaveTextContent('no longer exists');
    fireEvent.click(screen.getByRole('button', { name: 'Show every light' }));
    await waitFor(() => expect(where().get('flatGroup')).toBeNull());
  });

  // The request fails: the grid says the lights could not be found, not
  // "(updating)", and asks again later.
  it('says the lights could not be found rather than updating forever', async () => {
    let asked = 0;
    server.use(
      images(),
      http.get('/api/db/:dbId/calibrations/flat-groups/use', () => {
        asked += 1;
        return HttpResponse.json({ success: false, error: 'boom', status: 'error', data: null }, { status: 500 });
      }),
    );
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1') });
    await waitFor(() => expect(screen.getByText(/of 4 images/)).toBeInTheDocument());
    await waitFor(() => expect(stats()).toHaveTextContent('could not be found'));
    expect(stats()).not.toHaveTextContent('(updating)');
    expect(screen.getByTestId('flat-group-filter-state')).toHaveTextContent("Could not find the group's lights");
    expect(asked).toBeGreaterThan(0);
  });

  it('says the server could not work the lights out', async () => {
    server.use(
      images(),
      http.get('/api/db/:dbId/calibrations/flat-groups/use', () => ok({
        computed: false, groups: [], lights: 0, pending: 0, unread: 0, refreshing: false,
        error: 'the catalog could not be read',
      })),
    );
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&flatGroup=c925:g1') });
    await waitFor(() => expect(screen.getByTestId('flat-group-filter-state'))
      .toHaveTextContent('the catalog could not be read'));
  });
});
