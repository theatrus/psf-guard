import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import GroupedImageGrid from '../GroupedImageGrid';

const image = (id: number, acquired: string) => ({
  id,
  project_id: 1,
  project_name: 'Bubble',
  project_display_name: 'Bubble',
  target_id: 42,
  target_name: 'Bubble Nebula',
  acquired_date: Date.parse(acquired) / 1000,
  filter_name: 'R',
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: `${id}.fits` },
  filesystem_path: `/images/${id}.fits`,
});

const preferences = { showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' } as const;

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
          {children}
          <Where />
        </MemoryRouter>
      </QueryClientProvider>
    );
  };
}

describe('GroupedImageGrid date filter', () => {
  afterEach(() => setDisplayPreferences({ ...preferences }));

  it('keeps one night\'s window when another filter changes', async () => {
    // A catalog whose nights split at 06:00 UTC: the night of 2024-01-15
    // runs to 05:59 the next morning, and a frame at 03:00 on the 15th
    // belongs to the night before.
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, error: null, status: 'ready', data: [
          image(1, '2024-01-15T03:00:00Z'),
          image(2, '2024-01-15T21:00:00Z'),
          image(3, '2024-01-16T04:00:00Z'),
        ] })),
    );
    setDisplayPreferences({ ...preferences, showCalibrationChip: false });
    const window = 'dateStart=2024-01-15T06%3A00%3A00.000Z&dateEnd=2024-01-16T05%3A59%3A59.999Z';
    render(<GroupedImageGrid />, { wrapper: wrapper(`/grid?db=c925&project=1&${window}`) });
    await waitFor(() => expect(screen.getByText(/2 of 3 images/)).toBeInTheDocument());

    fireEvent.click(screen.getByRole('checkbox', { name: 'Rejected' }));
    await waitFor(() => expect(screen.getByTestId('where').textContent).toContain('status='));
    const params = new URLSearchParams(screen.getByTestId('where').textContent ?? '');
    expect(params.get('dateStart')).toBe('2024-01-15T06:00:00.000Z');
    expect(params.get('dateEnd')).toBe('2024-01-16T05:59:59.999Z');
    expect(screen.getByText(/2 of 3 images/)).toBeInTheDocument();
  });

  it('keeps a day picked in the date inputs as a day', async () => {
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, error: null, status: 'ready', data: [
          image(1, '2024-01-15T03:00:00Z'),
        ] })),
    );
    setDisplayPreferences({ ...preferences, showCalibrationChip: false });
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1') });
    await waitFor(() => expect(screen.getByText(/1 of 1 images/)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /^Dates/ }));
    fireEvent.change(screen.getByLabelText('Date range:'), { target: { value: '2024-01-15' } });
    await waitFor(() => expect(
      new URLSearchParams(screen.getByTestId('where').textContent ?? '').get('dateStart'),
    ).toBe('2024-01-15'));
  });
});
