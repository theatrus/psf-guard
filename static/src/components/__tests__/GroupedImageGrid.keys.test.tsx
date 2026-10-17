import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import GroupedImageGrid from '../GroupedImageGrid';

const image = (id: number) => ({
  id,
  project_id: 1,
  project_name: 'Bubble',
  project_display_name: 'Bubble',
  target_id: 42,
  target_name: 'Bubble Nebula',
  acquired_date: Date.parse('2026-10-01T20:00:00Z') / 1000 + id * 3600,
  filter_name: 'HA',
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: `${id}.fits` },
  filesystem_path: `/images/${id}.fits`,
});

function Where() {
  return <output data-testid="where">{useLocation().pathname}</output>;
}

function mount(route: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[route]}>
        <GroupedImageGrid />
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const card = (id: number) => document.querySelector(`[data-image-id="${id}"]`)!;

describe('GroupedImageGrid Enter and Space', () => {
  const originalScrollIntoView = Element.prototype.scrollIntoView;
  beforeEach(() => {
    Element.prototype.scrollIntoView = () => {};
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, error: null, status: 'ready', data: [image(1), image(2)] })),
    );
    // The grid's keys listen only on the live grid route.
    window.location.hash = '#/grid';
  });
  afterEach(() => {
    Element.prototype.scrollIntoView = originalScrollIntoView;
    window.location.hash = '';
  });

  it('leaves Enter and Space to a focused button, and still acts on the current card', async () => {
    mount('/grid?db=c925&project=1');
    await waitFor(() => expect(card(2)).toBeTruthy());
    fireEvent.click(card(2).querySelector('.image-card')!);
    await waitFor(() => expect(card(2)).toHaveClass('current-selection'));
    expect(card(2)).toHaveClass('multi-selected');

    // Enter and Space on View press View, not the grid.
    const view = screen.getByRole('button', { name: /^View/ });
    view.focus();
    fireEvent.keyDown(view, { key: ' ', code: 'Space' });
    expect(card(2)).toHaveClass('multi-selected');
    fireEvent.keyDown(view, { key: 'Enter', code: 'Enter' });
    expect(screen.getByTestId('where')).toHaveTextContent('/grid');

    view.blur();
    fireEvent.keyDown(document, { key: ' ', code: 'Space' });
    await waitFor(() => expect(card(2)).not.toHaveClass('multi-selected'));
    fireEvent.keyDown(document, { key: 'Enter', code: 'Enter' });
    await waitFor(() => expect(screen.getByTestId('where')).toHaveTextContent('/detail/2'));
  });
});
