import { describe, it, expect } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryRouter, RouterProvider, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import SequenceView from '../SequenceView';
import multiSessionFixture from '../../__fixtures__/sequence-analysis-multi-session.json';

/**
 * Sequence's detail link starts from the URL as it stands
 * (useLatestSearchParams): a selection changed a moment before the
 * double-click, not yet rendered, goes with it.
 */
function Where() {
  const { pathname, search } = useLocation();
  return <output data-testid="where">{pathname + search}</output>;
}

const where = () => screen.getByTestId('where').textContent ?? '';
const selectedOf = (url: string) =>
  new Set((new URLSearchParams(url.split('?')[1] ?? '').get('selected') ?? '').split(',').filter(Boolean));

describe('SequenceView detail link', () => {
  it('carries a selection changed a moment before the double-click', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => HttpResponse.json(multiSessionFixture)),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => HttpResponse.json({
        success: true,
        data: [{ id: 2, name: 'NGC7000', ra: 314, dec: 44, active: true, image_count: 10, accepted_count: 0, rejected_count: 0, has_files: true }],
        error: null,
        status: 'ready',
      })),
      http.get('/api/db/:dbId/images', () => HttpResponse.json({ success: true, data: [], error: null, status: 'ready' })),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const router = createMemoryRouter(
      [
        { path: '/sequence', element: <><SequenceView /><Where /></> },
        { path: '/detail/:id', element: <Where /> },
      ],
      { initialEntries: ['/sequence?db=test&project=1&target=2'] },
    );
    render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>);

    await waitFor(() => expect(document.querySelectorAll('.sequence-image-card').length).toBeGreaterThan(1));
    // Let the view settle its cursor and selection first.
    await waitFor(() => expect(where()).toContain('current='));
    const cards = document.querySelectorAll<HTMLElement>('.sequence-image-card');
    const toggled = cards[0].dataset.cardImageId!;
    const opened = cards[1].dataset.cardImageId!;
    const expected = selectedOf(where());
    if (expected.has(toggled)) expected.delete(toggled); else expected.add(toggled);

    act(() => {
      fireEvent.click(cards[0], { ctrlKey: true });
      fireEvent.doubleClick(cards[1]);
    });
    await waitFor(() => expect(where()).toMatch(new RegExp(`^/detail/${opened}\\?`)));
    expect(selectedOf(where())).toEqual(expected);
  });
});
