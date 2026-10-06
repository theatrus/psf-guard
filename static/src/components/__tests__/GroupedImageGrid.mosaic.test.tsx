import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import GroupedImageGrid from '../GroupedImageGrid';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });

const image = (id: number, targetId: number, targetName: string, acquired: number) => ({
  id,
  project_id: 1,
  project_name: 'M31',
  project_display_name: 'M31',
  target_id: targetId,
  target_name: targetName,
  acquired_date: acquired,
  filter_name: 'L',
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: `${id}.fits` },
  filesystem_path: `/images/${id}.fits`,
});

const panel = (targetId: number, name: string, row: number, column: number) => ({
  target_id: targetId, target_name: name, panel_id: `r${row}c${column}`, row, column,
});

function mount(route: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[route]}>{children}</MemoryRouter>
    </QueryClientProvider>
  );
  return render(<GroupedImageGrid />, { wrapper: Wrapper });
}

describe('GroupedImageGrid in a mosaic scope', () => {
  // jsdom has no layout; the grid scrolls the card it moves to into view.
  const originalScrollIntoView = Element.prototype.scrollIntoView;
  beforeEach(() => { Element.prototype.scrollIntoView = () => {}; });
  afterEach(() => {
    Element.prototype.scrollIntoView = originalScrollIntoView;
  });

  it("groups the frames under their panels in grid order, and arrows cross from one panel to the next", async () => {
    // Stored out of grid order, and newest first, so only the mosaic orders them.
    server.use(
      http.get('/api/db/:dbId/images', () => ok([
        image(1, 12, 'M31 bottom', 300),
        image(2, 11, 'M31 top right', 200),
        image(3, 10, 'M31 top left', 100),
        image(4, 10, 'M31 top left', 110),
      ])),
      http.get('/api/db/test/projects/1/mosaic', () => ok({ mosaic: {
        project_id: 1, name: 'M31', source: 'director', rows: 2, columns: 2,
        panels: [panel(10, 'M31 top left', 1, 1), panel(11, 'M31 top right', 1, 2), panel(12, 'M31 bottom', 2, 1)],
      } })),
    );
    mount('/grid?db=test&project=1&mosaic=1&grouping=filter');

    await waitFor(() => expect(document.querySelectorAll('.mosaic-panel-heading')).toHaveLength(3));
    expect([...document.querySelectorAll('.mosaic-panel-heading')].map(heading => heading.textContent)).toEqual([
      'r1c1 · M31 top left', 'r1c2 · M31 top right', 'r2c1 · M31 bottom',
    ]);
    expect(screen.getByText(/M31 mosaic \(3 panels\)/)).toBeInTheDocument();
    const order = () => [...document.querySelectorAll('[data-image-id]')].map(card => Number(card.getAttribute('data-image-id')));
    await waitFor(() => expect(order()).toEqual([3, 4, 2, 1]));

    // The last frame of the first panel steps into the second panel. The
    // grid's keys listen only on the live grid route.
    window.location.hash = '#/grid';
    fireEvent.click(document.querySelector('[data-image-id="4"] .image-card')!);
    await waitFor(() => expect(document.querySelector('[data-image-id="4"]')).toHaveClass('current-selection'));
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(document.querySelector('[data-image-id="2"]')).toHaveClass('current-selection'));
    fireEvent.keyDown(document, { key: ' ', code: 'Space' });
    await waitFor(() => expect(document.querySelector('[data-image-id="2"]')).toHaveClass('multi-selected'));
    window.location.hash = '';
  });

  it('reads a project that is not a mosaic as an ordinary grid', async () => {
    server.use(
      http.get('/api/db/:dbId/images', () => ok([image(1, 10, 'M81', 100)])),
    );
    mount('/grid?db=test&project=1&mosaic=1');
    await waitFor(() => expect(document.querySelectorAll('[data-image-id]')).toHaveLength(1));
    expect(document.querySelector('.mosaic-panel-heading')).toBeNull();
  });
});
