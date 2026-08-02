import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import GroupedImageGrid from '../GroupedImageGrid';

const image = {
  id: 7,
  project_id: 1,
  project_name: 'Test Project',
  project_display_name: 'Test Project',
  target_id: 42,
  target_name: 'Sh2 86',
  acquired_date: 1_705_352_400,
  filter_name: 'R',
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: 'sh2-86-r.fits' },
  filesystem_path: '/images/sh2-86-r.fits',
};

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

function scanStatus(running: boolean) {
  return {
    success: true,
    data: {
      started: running,
      progress: {
        running,
        stage: running ? 'spatial' : '',
        target_id: running ? 42 : null,
        filter_name: running ? 'R' : null,
        total: running ? 10 : 0,
        processed: running ? 1 : 0,
        skipped_cached: 0,
        spatial_processed: 0,
        astrometry_processed: 0,
        solved: 0,
        solve_failed: 0,
        operational_errors: 0,
        errors: 0,
        current_file: running ? 'sh2-86-r.fits' : null,
        started_at: running ? 1_705_352_400 : null,
        finished_at: null,
        last_error: null,
      },
      cached_count: 0,
    },
    error: null,
    status: 'ready',
  };
}

function imagesResponse() {
  return {
    success: true,
    data: [image],
    error: null,
    status: 'ready',
  };
}

describe('GroupedImageGrid quality analysis', () => {
  it('starts analysis for the selected target and filter', async () => {
    let posted: unknown = null;
    server.use(
      http.get('/api/db/:dbId/images', () => HttpResponse.json(imagesResponse())),
      http.post('/api/db/:dbId/analysis/quality-scan', async ({ request }) => {
        posted = await request.json();
        return HttpResponse.json(scanStatus(true));
      }),
    );

    render(<GroupedImageGrid />, {
      wrapper: wrapper('/grid?db=test&project=1&target=42&filter=R'),
    });

    const button = await screen.findByRole('button', { name: 'Analyze Quality' });
    await userEvent.click(button);

    await waitFor(() => {
      expect(posted).toMatchObject({ target_id: 42, filter_name: 'R' });
    });
    expect(await screen.findByRole('button', { name: 'Analysis running…' })).toBeDisabled();
  });

  it('asks for a target before starting a project-wide grid scan', async () => {
    server.use(
      http.get('/api/db/:dbId/images', () => HttpResponse.json(imagesResponse())),
    );

    render(<GroupedImageGrid />, {
      wrapper: wrapper('/grid?db=test&project=1'),
    });

    const button = await screen.findByRole('button', { name: 'Analyze Quality' });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute(
      'title',
      'Choose a target from the header before starting quality analysis.',
    );
  });
});
