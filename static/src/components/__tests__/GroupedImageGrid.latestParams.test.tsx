import { afterEach, describe, expect, it } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryRouter, RouterProvider, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import GroupedImageGrid from '../GroupedImageGrid';

/**
 * Review repro (PR #872, 27c6ae60): pins one useLatestSearchParams
 * conversion. A selection written a moment before a double-click, not yet
 * rendered, goes with the detail link. Reverting useLatestSearchParams to
 * the rendered search leaves the rest of the suite green.
 */
const image = (id: number) => ({
  id, project_id: 1, project_name: 'Bubble', project_display_name: 'Bubble',
  target_id: 42, target_name: 'Bubble Nebula',
  acquired_date: Date.parse('2024-01-15T21:00:00Z') / 1000 + id,
  filter_name: 'R', grading_status: 0, reject_reason: null,
  metadata: { FileName: `${id}.fits` }, filesystem_path: `/images/${id}.fits`,
});

const preferences = { showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' } as const;

function Where() {
  const { pathname, search } = useLocation();
  return <output data-testid="where">{pathname + search}</output>;
}

describe('GroupedImageGrid detail link', () => {
  afterEach(() => setDisplayPreferences({ ...preferences }));

  it('carries a selection made a moment before the double-click', async () => {
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, error: null, status: 'ready', data: [image(1), image(2)] })),
    );
    setDisplayPreferences({ ...preferences, showCalibrationChip: false });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const router = createMemoryRouter(
      [
        { path: '/grid', element: <><GroupedImageGrid /><Where /></> },
        { path: '/detail/:id', element: <Where /> },
      ],
      { initialEntries: ['/grid?db=c925&project=1'] },
    );
    render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>);
    await waitFor(() => expect(document.querySelectorAll('.image-card')).toHaveLength(2));
    const cards = document.querySelectorAll<HTMLElement>('.image-card');

    act(() => {
      fireEvent.click(cards[0], { ctrlKey: true });
      fireEvent.doubleClick(cards[1]);
    });
    await waitFor(() => expect(screen.getByTestId('where').textContent).toMatch(/^\/detail\/2\?/));
    const params = new URLSearchParams(screen.getByTestId('where').textContent!.split('?')[1]);
    expect(params.get('selected')).toBe('1');
  });
});
