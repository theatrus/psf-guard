import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import GroupedImageGrid from '../GroupedImageGrid';

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
  return <output data-testid="where">{`${location.pathname}${location.search}`}</output>;
}

function wrapper(route: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>
          <Routes>
            <Route path="/grid" element={children} />
            <Route path="/detail/:imageId" element={null} />
          </Routes>
          <Where />
        </MemoryRouter>
      </QueryClientProvider>
    );
  };
}

const preferences = { showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' } as const;

function serve(asked: string[] = []) {
  server.use(
    http.get('/api/db/:dbId/images', () =>
      HttpResponse.json({ success: true, error: null, status: 'ready', data: [image(7), image(8), image(9, 'G')] })),
    http.get('/api/db/:dbId/calibration-health/lights', ({ request }) => {
      asked.push(new URL(request.url).search);
      return HttpResponse.json({ success: true, error: null, status: 'ready', data: {
        lights: [
          { image_id: 8, level: 'poor', codes: ['cannot_calibrate', 'old_flats'] },
          { image_id: 9, level: 'check', codes: ['old_flats'] },
        ],
        stale_projects: [],
        running: false,
      } });
    }),
  );
}

describe('GroupedImageGrid calibration marks', () => {
  afterEach(() => setDisplayPreferences({ ...preferences }));

  it('keeps the lights the library serves poorly and marks them with a chip', async () => {
    const asked: string[] = [];
    serve(asked);
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&calib=poor') });
    await waitFor(() => expect(screen.getByText(/1 of 3 images/)).toBeInTheDocument());
    expect(document.querySelector('.grid-stats')).toHaveTextContent('Poor');
    expect(asked).toContain('?project_id=1');
    const chip = screen.getByTestId('calibration-chip');
    expect(chip).toHaveClass('is-poor');
    expect(chip).toHaveAttribute(
      'title',
      "Can't be calibrated properly: Can't be calibrated, Old flats. Open its calibration.",
    );

    const button = screen.getByRole('button', { name: /^Calibration:/ });
    fireEvent.click(button);
    const menu = screen.getByRole('group', { name: 'Calibration' });
    fireEvent.click(within(menu).getByRole('checkbox', { name: 'Check' }));
    await waitFor(() => expect(screen.getByText(/2 of 3 images/)).toBeInTheDocument());
    expect(screen.getAllByTestId('calibration-chip').map((element) => element.className).sort()).toEqual([
      'score-chip calibration-chip is-check',
      'score-chip calibration-chip is-poor',
    ]);
  });

  it('opens a light on its calibration account from the chip, without selecting it', async () => {
    serve();
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&calib=poor') });
    await waitFor(() => expect(screen.getByText(/1 of 3 images/)).toBeInTheDocument());
    await userEvent.click(screen.getByRole('button', { name: /Open its calibration/ }));
    await waitFor(() => expect(screen.getByTestId('where')).toHaveTextContent('/detail/8'));
    const where = new URL(screen.getByTestId('where').textContent!, 'http://x');
    expect(where.pathname).toBe('/detail/8');
    expect(where.searchParams.get('calibFocus')).toBe('cannot_calibrate');
    expect(where.searchParams.get('calib')).toBe('poor');
    expect(where.searchParams.get('selected')).toBeNull();
  });

  it('offers the finding a coverage report link chose, beside the two levels', async () => {
    serve();
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&calib=cannot_calibrate&filter=R') });
    await waitFor(() => expect(screen.getByText(/1 of 3 images/)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /^Calibration:/ }));
    const menu = screen.getByRole('group', { name: 'Calibration' });
    expect(within(menu).getAllByRole('checkbox').map((box) => box.closest('label')!.textContent)).toEqual([
      'All',
      'Poor',
      'Check',
      "Can't be calibrated",
    ]);
    expect(within(menu).getByRole('checkbox', { name: "Can't be calibrated" })).toBeChecked();
  });

  it('reads no marks when the chip is off and nothing is filtered by calibration', async () => {
    const asked: string[] = [];
    serve(asked);
    setDisplayPreferences({ ...preferences, showCalibrationChip: false });
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1') });
    await waitFor(() => expect(screen.getByText(/3 of 3 images/)).toBeInTheDocument());
    expect(screen.queryByTestId('calibration-chip')).not.toBeInTheDocument();
    expect(asked).toEqual([]);
  });
});
