import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import GroupedImageGrid from '../GroupedImageGrid';

const image = (id: number, file: string) => ({
  id,
  project_id: 1,
  project_name: 'Test Project',
  project_display_name: 'Test Project',
  target_id: 42,
  target_name: 'Sh2 86',
  acquired_date: 1_705_352_400 + id,
  filter_name: 'R',
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: file },
  filesystem_path: `/images/${file}`,
});

function wrapper(route: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[route]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

const quality = (imageId: number, flags: string[]) => ({
  image_id: imageId,
  quality_score: flags.length ? 0.3 : 0.9,
  category: flags[0] ?? null,
  flags,
  normalized_metrics: {},
  temporal_deviation: 0,
  details: null,
  regrade_reason: flags.length ? '[Auto] Astrometry: Rotation skew - score 0.30; +5.0° from the sequence' : null,
});

const analysis = {
  success: true,
  data: {
    sequences: [
      {
        target_id: 42,
        target_name: 'Sh2 86',
        filter_name: 'R',
        session_start: 1_705_352_400,
        session_end: 1_705_352_402,
        image_count: 2,
        reference_values: {},
        images: [quality(7, []), quality(8, ['rotation_skew'])],
        summary: {},
      },
    ],
    target_filter_rollups: [],
  },
  error: null,
  status: 'ready',
};

describe('GroupedImageGrid flag filter', () => {
  it('keeps only the frames whose quality analysis raised the chosen flag', async () => {
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, data: [image(7, 'a.fits'), image(8, 'b.fits')], error: null, status: 'ready' })
      ),
      http.get('/api/db/:dbId/analysis/sequence', () => HttpResponse.json(analysis)),
    );
    render(<GroupedImageGrid />, {
      wrapper: wrapper('/grid?db=test&project=1&target=42&flag=rotation_skew'),
    });
    await waitFor(() => expect(screen.getByText(/1 of 2 images/)).toBeInTheDocument());
    const select = screen.getByLabelText('Flag:') as HTMLSelectElement;
    expect(select.value).toBe('rotation_skew');
    expect(Array.from(select.options).map((option) => option.textContent)).toEqual([
      'All',
      'Rotation Skew',
    ]);
  });

  it('hides nothing while the analysis is loading or when it fails', async () => {
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, data: [image(7, 'a.fits'), image(8, 'b.fits')], error: null, status: 'ready' })
      ),
      http.get('/api/db/:dbId/analysis/sequence', () => HttpResponse.json({ success: false, data: null, error: 'boom' }, { status: 500 })),
    );
    render(<GroupedImageGrid />, {
      wrapper: wrapper('/grid?db=test&project=1&target=42&flag=rotation_skew'),
    });
    await waitFor(() => expect(screen.getByText(/2 of 2 images/)).toBeInTheDocument());
  });
});
