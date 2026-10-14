import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse, type JsonBodyType } from 'msw';
import { server } from '../../test/msw-server';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import { PROJECT_SEEN_STORAGE_KEY, projectSeenKey } from '../../utils/projectRecency';
import SequenceView from '../SequenceView';
import normalFixture from '../../__fixtures__/sequence-analysis-normal.json';
import multiSessionFixture from '../../__fixtures__/sequence-analysis-multi-session.json';

const START = 1705352400;

// The normal fixture's frames 1 to 10, five minutes apart.
const images = normalFixture.data.sequences[0].images.map((frame, index) => ({
  id: frame.image_id,
  project_id: 1,
  project_name: 'Test Project',
  project_display_name: 'Test Project',
  target_id: 1,
  target_name: 'M42',
  acquired_date: START + index * 300,
  filter_name: 'L',
  grading_status: 0,
  reject_reason: null,
  metadata: {},
  filesystem_path: null,
}));

const preferences = { showNightChip: true, showAllChip: true, showCalibrationChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' } as const;

function Where() {
  return <output data-testid="where">{useLocation().search}</output>;
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

function handlers(analysis: JsonBodyType = normalFixture) {
  server.use(
    http.get('/api/db/:dbId/analysis/sequence', () => HttpResponse.json(analysis)),
    http.get('/api/db/:dbId/projects/:projectId/targets', () => HttpResponse.json({
      success: true, error: null, status: 'ready',
      data: [{ id: 1, name: 'M42', ra: 83.82, dec: -5.39, active: true, image_count: 10, accepted_count: 0, rejected_count: 0, has_files: true }],
    })),
    http.get('/api/db/:dbId/images', () =>
      HttpResponse.json({ success: true, error: null, status: 'ready', data: images })),
  );
}

const params = () => new URLSearchParams(screen.getByTestId('where').textContent ?? '');
const stripIds = () => Array.from(document.querySelectorAll('.sequence-image-card'))
  .map((card) => Number(card.getAttribute('data-card-image-id')));
const chartIds = () => Array.from(document.querySelectorAll('.sequence-timeline rect[data-image-id]'))
  .map((bar) => Number(bar.getAttribute('data-image-id')));
const card = (id: number) => document.querySelector(`.sequence-image-card[data-card-image-id="${id}"]`)!;

describe('SequenceView order', () => {
  afterEach(() => {
    window.localStorage.removeItem(PROJECT_SEEN_STORAGE_KEY);
    setDisplayPreferences({ ...preferences });
  });

  it('shows the strip newest first and keeps the chart in capture order', async () => {
    handlers();
    render(<SequenceView />, { wrapper: wrapper('/sequence?db=test&project=1&target=1&order=newest') });
    await waitFor(() => expect(stripIds()).toEqual([10, 9, 8, 7, 6, 5, 4, 3, 2, 1]));
    expect(chartIds()).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    // It opens on the first frame it shows.
    await waitFor(() => expect(card(10)).toHaveClass('current-selection'));
  });

  it('moves the cursor, Space and Shift-click in the order shown', async () => {
    handlers();
    render(<SequenceView />, { wrapper: wrapper('/sequence?db=test&project=1&target=1&order=newest') });
    await waitFor(() => expect(card(10)).toHaveClass('current-selection'));

    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(card(9)).toHaveClass('current-selection'));
    fireEvent.keyDown(document, { key: ' ', code: 'Space' });
    expect(card(9)).toHaveClass('selected');

    fireEvent.click(card(7), { shiftKey: true });
    await waitFor(() => expect(document.querySelectorAll('.sequence-image-card.selected')).toHaveLength(3));
    for (const id of [9, 8, 7]) expect(card(id)).toHaveClass('selected');
  });

  it('turns around from the View panel without losing the cursor or the selection', async () => {
    handlers();
    render(<SequenceView />, { wrapper: wrapper('/sequence?db=test&project=1&target=1&current=4&selected=4,5') });
    await waitFor(() => expect(stripIds()).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
    await waitFor(() => expect(card(4)).toHaveClass('current-selection'));

    fireEvent.click(screen.getByRole('button', { name: 'View' }));
    fireEvent.click(within(screen.getByRole('radiogroup', { name: 'Sort' }))
      .getByRole('radio', { name: 'Newest first' }));
    await waitFor(() => expect(stripIds()).toEqual([10, 9, 8, 7, 6, 5, 4, 3, 2, 1]));
    expect(params().get('order')).toBe('newest');
    expect(params().get('current')).toBe('4');
    expect(card(4)).toHaveClass('current-selection');
    expect(card(5)).toHaveClass('selected');
    expect(chartIds()).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    expect(screen.getByRole('button', { name: 'View (newest first)' })).toHaveClass('is-set');
  });

  it('at rest, flips onto the new first frame; once moved, keeps the cursor', async () => {
    handlers();
    render(<SequenceView />, { wrapper: wrapper('/sequence?db=test&project=1&target=1') });
    await waitFor(() => expect(card(1)).toHaveClass('current-selection'));

    fireEvent.click(screen.getByRole('button', { name: 'View' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Newest first' }));
    await waitFor(() => expect(stripIds()[0]).toBe(10));
    await waitFor(() => expect(card(10)).toHaveClass('current-selection'));
    expect(params().get('current')).toBe('10');

    // An arrow moves the cursor; the next flip keeps it.
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(card(9)).toHaveClass('current-selection'));
    fireEvent.click(screen.getByRole('radio', { name: 'Oldest first' }));
    await waitFor(() => expect(stripIds()[0]).toBe(1));
    expect(params().get('current')).toBe('9');
    expect(card(9)).toHaveClass('current-selection');
  });

  it('lists the session tabs newest first, each filter\'s all-sessions tab still first', async () => {
    const [first, second] = multiSessionFixture.data.sequences;
    handlers({
      ...multiSessionFixture,
      data: {
        ...multiSessionFixture.data,
        target_filter_rollups: [{
          target_id: first.target_id,
          target_name: first.target_name,
          filter_name: first.filter_name,
          session_start: first.session_start,
          session_end: second.session_end,
          image_count: 10,
          unavailable_image_count: 0,
          images: [...first.images, ...second.images].map((frame) => ({
            image_id: frame.image_id,
            quality_score: frame.quality_score,
            normalized_metrics: frame.normalized_metrics,
            details: frame.details,
          })),
          summary: first.summary,
        }],
      },
    });
    render(<SequenceView />, { wrapper: wrapper('/sequence?db=test&project=1&target=1&order=newest') });
    const tabs = await screen.findAllByRole('button', { name: /^L · / });
    expect(tabs).toHaveLength(3);
    expect(tabs[0]).toHaveTextContent('L · All sessions (10)');
    // The later session leads, and opens on its newest frame.
    fireEvent.click(tabs[1]);
    await waitFor(() => expect(stripIds()).toEqual([215, 214, 213, 212, 211]));
    await waitFor(() => expect(params().get('current')).toBe('215'));
    fireEvent.click(tabs[2]);
    await waitFor(() => expect(stripIds()).toEqual([205, 204, 203, 202, 201]));
  });

  it('marks the frames captured since the last visit from the Library', async () => {
    window.localStorage.setItem(PROJECT_SEEN_STORAGE_KEY, JSON.stringify({
      [projectSeenKey('test', 1)]: {
        latestImage: START + 6 * 300,
        totalImages: 7,
        targets: { 1: { latestImage: START + 6 * 300, totalImages: 7 } },
      },
    }));
    handlers();
    render(<SequenceView />, { wrapper: wrapper('/sequence?db=test&project=1&target=1') });
    await waitFor(() => expect(stripIds()).toHaveLength(10));
    await waitFor(() => expect(document.querySelectorAll('.image-new-mark')).toHaveLength(3));
    for (const id of [8, 9, 10]) expect(card(id).querySelector('.image-new-mark')).not.toBeNull();
    expect(card(7).querySelector('.image-new-mark')).toBeNull();
  });
});
