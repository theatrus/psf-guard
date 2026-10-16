import { afterEach, describe, expect, it } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryRouter, RouterProvider, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import GroupedImageGrid from '../GroupedImageGrid';

const image = (id: number, filter: string) => ({
  id,
  project_id: 1,
  project_name: 'Bubble',
  project_display_name: 'Bubble',
  target_id: 42,
  target_name: 'Bubble Nebula',
  acquired_date: Date.parse('2024-01-15T21:00:00Z') / 1000 + id,
  filter_name: filter,
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: `${id}.fits` },
  filesystem_path: `/images/${id}.fits`,
});

const preferences = { showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' } as const;

function Where() {
  return <output data-testid="where">{useLocation().search}</output>;
}

describe('GroupedImageGrid expanded groups', () => {
  afterEach(() => setDisplayPreferences({ ...preferences }));

  it('keeps both of two groups toggled before the grid renders again', async () => {
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, error: null, status: 'ready', data: [image(1, 'R'), image(2, 'G')] })),
    );
    setDisplayPreferences({ ...preferences, showCalibrationChip: false });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    // The app's kind of router.
    const router = createMemoryRouter(
      [{ path: '/grid', element: <><GroupedImageGrid /><Where /></> }],
      { initialEntries: ['/grid?db=c925&project=1&grouping=filter'] },
    );
    render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>);
    const red = await screen.findByRole('heading', { name: 'R' });
    const green = screen.getByRole('heading', { name: 'G' });

    // Both open at first; close both in one go.
    act(() => {
      fireEvent.click(red);
      fireEvent.click(green);
    });
    await waitFor(() => expect(screen.getByTestId('where').textContent).toContain('expanded='));
    expect(new URLSearchParams(screen.getByTestId('where').textContent ?? '').get('expanded')).toBe('__none__');
  });
});
