import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import SequenceView from '../SequenceView';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });

const image = (id: number, targetId: number, acquired: number, filter = 'L') => ({
  id,
  project_id: 1,
  project_name: 'Heart',
  project_display_name: 'Heart',
  target_id: targetId,
  target_name: `Heart ${targetId}`,
  acquired_date: acquired,
  filter_name: filter,
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: `${id}.fits` },
  filesystem_path: `/images/${id}.fits`,
});

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}{location.search}</output>;
}

function mount(route: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter>
    </QueryClientProvider>
  );
  return render(<SequenceView />, { wrapper: Wrapper });
}

describe("a mosaic's sequence", () => {
  it('interleaves every panel by capture time, names each frame by its panel, and opens a panel', async () => {
    server.use(
      http.get('/api/db/:dbId/images', () => ok([
        image(1, 20, 400), image(2, 21, 100), image(3, 20, 200), image(4, 21, 300, 'R'),
      ])),
      http.get('/api/db/test/projects/1/mosaic', () => ok({ mosaic: {
        project_id: 1, name: 'Heart', source: 'inferred', rows: 1, columns: 2,
        panels: [
          { target_id: 20, target_name: 'Heart 20', panel_id: 'r1c1', row: 1, column: 1 },
          { target_id: 21, target_name: 'Heart 21', panel_id: 'r1c2', row: 1, column: 2 },
        ],
      } })),
    );
    mount('/sequence?db=test&project=1&mosaic=1');

    const frames = await screen.findByRole('list', { name: 'Frames in capture order' });
    expect(screen.getByRole('heading', { name: 'Heart mosaic (2 panels)' })).toBeInTheDocument();
    const items = within(frames).getAllByRole('listitem');
    expect(items.map(item => item.getAttribute('data-image-id'))).toEqual(['2', '3', '4', '1']);
    expect(items.map(item => item.querySelector('.mosaic-sequence-panel')?.textContent)).toEqual(['r1c2', 'r1c1', 'r1c2', 'r1c1']);
    // It stays on the mosaic rather than narrowing to the first frame's target.
    expect(screen.getByTestId('location')).toHaveTextContent('mosaic=1');

    fireEvent.change(screen.getByLabelText('Filter:'), { target: { value: 'R' } });
    await waitFor(() => expect(within(frames).getAllByRole('listitem')).toHaveLength(1));

    fireEvent.click(within(screen.getByRole('group', { name: "Open a panel's sequence" })).getByRole('button', { name: /r1c2/ }));
    expect(screen.getByTestId('location')).toHaveTextContent('target=21');
    expect(screen.getByTestId('location')).not.toHaveTextContent('mosaic=1');
  });
});
