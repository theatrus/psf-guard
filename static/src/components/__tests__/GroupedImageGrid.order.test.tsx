import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import {
  PROJECT_SEEN_STORAGE_KEY,
  projectSeenKey,
} from '../../utils/projectRecency';
import GroupedImageGrid from '../GroupedImageGrid';

const at = (hour: number) => Date.parse(`2026-10-01T${String(hour).padStart(2, '0')}:00:00Z`) / 1000;

const image = (id: number, hour: number, filter: string) => ({
  id,
  project_id: 1,
  project_name: 'Bubble',
  project_display_name: 'Bubble',
  target_id: 42,
  target_name: 'Bubble Nebula',
  acquired_date: at(hour),
  filter_name: filter,
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: `${id}.fits` },
  filesystem_path: `/images/${id}.fits`,
});

// Ha 1, 3, 5 and OIII 2, 4, an hour apart.
const frames = [
  image(1, 20, 'HA'),
  image(2, 21, 'OIII'),
  image(3, 22, 'HA'),
  image(4, 23, 'OIII'),
  image(5, 23, 'HA'),
].map((frame, index) => index === 4 ? { ...frame, acquired_date: at(23) + 1800 } : frame);

const preferences = { showNightChip: true, showAllChip: true, showCalibrationChip: false, advanceOnGrade: true, projectPickerGrouping: 'activity', libraryDensity: 'compact' } as const;

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

const params = () => new URLSearchParams(screen.getByTestId('where').textContent ?? '');
const cardIds = () => Array.from(document.querySelectorAll('[data-image-id]'))
  .map((card) => Number(card.getAttribute('data-image-id')));

describe('GroupedImageGrid order', () => {
  beforeEach(() => {
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, error: null, status: 'ready', data: frames })),
    );
    setDisplayPreferences({ ...preferences });
  });
  afterEach(() => {
    window.localStorage.removeItem(PROJECT_SEEN_STORAGE_KEY);
    setDisplayPreferences({ ...preferences });
  });

  it('turns each group around from the View panel, keeping the cursor and the selection', async () => {
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&current=3&selected=3,4') });
    await waitFor(() => expect(cardIds()).toEqual([1, 3, 5, 2, 4]));

    fireEvent.click(screen.getByRole('button', { name: 'View' }));
    const sort = screen.getByRole('radiogroup', { name: 'Sort' });
    expect(within(sort).getByRole('radio', { name: 'Oldest first' })).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(within(sort).getByRole('radio', { name: 'Newest first' }));

    // The filter groups stay A to Z; the frames in each run newest first.
    await waitFor(() => expect(cardIds()).toEqual([5, 3, 1, 4, 2]));
    expect(params().get('order')).toBe('newest');
    expect(params().get('current')).toBe('3');
    expect(params().get('selected')).toBe('3,4');
    expect(document.querySelector('[data-image-id="3"]')).toHaveClass('current-selection');
    expect(within(sort).getByRole('radio', { name: 'Newest first' })).toHaveAttribute('aria-checked', 'true');
    // The View button says the order is off its default.
    const view = screen.getByRole('button', { name: 'View (newest first)' });
    expect(view).toHaveClass('is-set');

    fireEvent.click(within(sort).getByRole('radio', { name: 'Oldest first' }));
    await waitFor(() => expect(cardIds()).toEqual([1, 3, 5, 2, 4]));
    expect(params().has('order')).toBe(false);
    expect(screen.getByRole('button', { name: 'View' })).not.toHaveClass('is-set');
  });

  it('at rest, flips with the page in place and the cursor on the new first card', async () => {
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1') });
    await waitFor(() => expect(cardIds()).toEqual([1, 3, 5, 2, 4]));
    await waitFor(() => expect(params().get('current')).toBe('1'));

    fireEvent.click(screen.getByRole('button', { name: 'View' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Newest first' }));
    await waitFor(() => expect(cardIds()).toEqual([5, 3, 1, 4, 2]));
    expect(params().get('order')).toBe('newest');
    expect(params().get('current')).toBe('5');
    expect(document.querySelector('[data-image-id="5"]')).toHaveClass('current-selection');
    expect(document.querySelector('[data-image-id="1"]')).not.toHaveClass('current-selection');

    // Back again, still at rest: the cursor follows the first card.
    fireEvent.click(screen.getByRole('radio', { name: 'Oldest first' }));
    await waitFor(() => expect(cardIds()).toEqual([1, 3, 5, 2, 4]));
    expect(params().get('current')).toBe('1');
  });

  it('once the person has clicked a card, even the first, the flip keeps it', async () => {
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1') });
    await waitFor(() => expect(cardIds()).toEqual([1, 3, 5, 2, 4]));
    fireEvent.click(document.querySelector('[data-image-id="1"] .image-card')!);

    fireEvent.click(screen.getByRole('button', { name: 'View' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Newest first' }));
    await waitFor(() => expect(cardIds()).toEqual([5, 3, 1, 4, 2]));
    expect(params().get('current')).toBe('1');
    expect(document.querySelector('[data-image-id="1"]')).toHaveClass('current-selection');
  });

  it('moves between the two orders with the arrow keys', async () => {
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1') });
    await waitFor(() => expect(cardIds()).toEqual([1, 3, 5, 2, 4]));
    fireEvent.click(screen.getByRole('button', { name: 'View' }));
    const oldest = screen.getByRole('radio', { name: 'Oldest first' });
    fireEvent.keyDown(oldest, { key: 'ArrowRight' });
    await waitFor(() => expect(cardIds()).toEqual([5, 3, 1, 4, 2]));
    expect(screen.getByRole('radio', { name: 'Newest first' })).toHaveFocus();
  });

  it('opens a link that names newest first that way, with the first visible frame current', async () => {
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&order=newest') });
    await waitFor(() => expect(cardIds()).toEqual([5, 3, 1, 4, 2]));
    await waitFor(() => expect(params().get('current')).toBe('5'));
  });

  it('selects a Shift-click range as the frames are shown', async () => {
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1&order=newest') });
    await waitFor(() => expect(cardIds()).toEqual([5, 3, 1, 4, 2]));
    const card = (id: number) => document.querySelector(`[data-image-id="${id}"] .image-card`)!;

    fireEvent.click(card(1));
    fireEvent.click(card(4), { shiftKey: true });
    // Between 1 and 4 as shown there is nothing: in capture order the
    // same click would take 1, 3, 5, 2 and 4.
    await waitFor(() => expect(params().get('selected')).toBe('1,4'));
  });

  it('marks only the frames captured since the last visit from the Library', async () => {
    // The Library last saw this project with its newest frame at 22:00.
    window.localStorage.setItem(PROJECT_SEEN_STORAGE_KEY, JSON.stringify({
      [projectSeenKey('c925', 1)]: { latestImage: at(22), totalImages: 3, targets: {} },
    }));
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1') });
    await waitFor(() => expect(cardIds()).toEqual([1, 3, 5, 2, 4]));

    const marked = Array.from(document.querySelectorAll('.image-new-mark'))
      .map((mark) => Number(mark.closest('[data-image-id]')!.getAttribute('data-image-id')));
    expect(marked.sort()).toEqual([4, 5]);
    expect(screen.getAllByText('New,')).toHaveLength(2);
    expect(screen.getAllByTitle('New since your last visit from the Library')).toHaveLength(2);
  });

  it('marks nothing in a project the Library has not listed in this browser', async () => {
    render(<GroupedImageGrid />, { wrapper: wrapper('/grid?db=c925&project=1') });
    await waitFor(() => expect(cardIds()).toHaveLength(5));
    expect(document.querySelectorAll('.image-new-mark')).toHaveLength(0);
  });
});
