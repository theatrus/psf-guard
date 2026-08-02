import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import DatabaseActivityStatus from '../DatabaseActivityStatus';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/grid?db=test&project=1&target=42']}>
          {children}
        </MemoryRouter>
      </QueryClientProvider>
    );
  };
}

describe('DatabaseActivityStatus quality activity', () => {
  it('shows the active target scan in the global database status slot', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/quality-scan', () => HttpResponse.json({
        success: true,
        data: {
          started: false,
          progress: {
            running: true,
            stage: 'astrometry',
            target_id: 42,
            filter_name: null,
            total: 10,
            processed: 4,
            skipped_cached: 0,
            spatial_processed: 10,
            astrometry_processed: 4,
            solved: 3,
            solve_failed: 1,
            operational_errors: 0,
            errors: 0,
            current_file: 'sh2-86-r-004.fits',
            started_at: 1_705_352_400,
            finished_at: null,
            last_error: null,
          },
          cached_count: 6,
        },
        error: null,
        status: 'ready',
      })),
    );

    render(<DatabaseActivityStatus />, { wrapper: wrapper() });

    expect(await screen.findByText('Analyzing quality')).toBeInTheDocument();
    expect(screen.getByText(/Solving 4\/10 frames · sh2-86-r-004\.fits/)).toBeInTheDocument();
  });

  it('moves completion and scan errors into the global status', async () => {
    let polls = 0;
    server.use(
      http.get('/api/db/:dbId/analysis/quality-scan', () => {
        polls += 1;
        const running = polls === 1;
        return HttpResponse.json({
          success: true,
          data: {
            started: false,
            progress: {
              running,
              stage: running ? 'astrometry' : 'complete',
              target_id: 42,
              filter_name: null,
              total: 10,
              processed: running ? 4 : 10,
              skipped_cached: 0,
              spatial_processed: 10,
              astrometry_processed: running ? 4 : 10,
              solved: running ? 3 : 8,
              solve_failed: running ? 1 : 2,
              operational_errors: 0,
              errors: running ? 0 : 1,
              current_file: running ? 'sh2-86-r-004.fits' : null,
              started_at: 1_705_352_400,
              finished_at: running ? null : 1_705_352_500,
              last_error: running ? null : 'Could not read sh2-86-r-010.fits',
            },
            cached_count: 10,
          },
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<DatabaseActivityStatus />, { wrapper: wrapper() });

    expect(await screen.findByText('Analyzing quality')).toBeInTheDocument();
    const completed = await screen.findByText('Quality analysis finished with errors', {}, {
      timeout: 3000,
    });
    expect(completed.closest('.quality-analysis-status')).toHaveAttribute(
      'title',
      'Could not read sh2-86-r-010.fits',
    );
    expect(screen.getByText('10/10 frames · 1 error')).toBeInTheDocument();
  });

  it('keeps database backfill progress while its target scan runs', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/quality-backfill', () => HttpResponse.json({
        success: true,
        data: {
          started: false,
          progress: {
            running: true,
            force: false,
            total_targets: 5,
            processed_targets: 2,
            current_target_id: 42,
            started_at: 1_705_352_400,
            finished_at: null,
          },
        },
        error: null,
        status: 'ready',
      })),
      http.get('/api/db/:dbId/analysis/quality-scan', () => HttpResponse.json({
        success: true,
        data: {
          started: false,
          progress: {
            running: true,
            stage: 'spatial',
            target_id: 42,
            filter_name: null,
            total: 10,
            processed: 4,
            skipped_cached: 0,
            spatial_processed: 4,
            astrometry_processed: 0,
            solved: 0,
            solve_failed: 0,
            operational_errors: 0,
            errors: 0,
            current_file: 'sh2-86-r-004.fits',
            started_at: 1_705_352_400,
            finished_at: null,
            last_error: null,
          },
          cached_count: 4,
        },
        error: null,
        status: 'ready',
      })),
    );

    render(<DatabaseActivityStatus />, { wrapper: wrapper() });

    expect(await screen.findByText('Analyzing database quality')).toBeInTheDocument();
    expect(screen.getByText(/2\/5 targets · Scanning 4\/10 frames/)).toBeInTheDocument();
  });
});
