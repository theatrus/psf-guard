import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import AggregatedCacheStatus from '../AggregatedCacheStatus';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

describe('AggregatedCacheStatus quality activity', () => {
  it('keeps a quality scan visible outside a database-scoped view', async () => {
    server.use(
      http.get('/api/databases', () => HttpResponse.json({
        success: true,
        data: [{ id: 'test', name: 'Demo catalog', path: '/demo.sqlite' }],
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
            processed: 5,
            skipped_cached: 0,
            spatial_processed: 5,
            astrometry_processed: 0,
            solved: 0,
            solve_failed: 0,
            operational_errors: 0,
            errors: 0,
            current_file: 'sh2-86-r-005.fits',
            started_at: 1_705_352_400,
            finished_at: null,
            last_error: null,
          },
          cached_count: 5,
        },
        error: null,
        status: 'ready',
      })),
    );

    render(<AggregatedCacheStatus />, { wrapper: wrapper() });

    const summary = await screen.findByText('Working on 1 of 1 database');
    await userEvent.click(summary.closest('.cache-refresh-status')!);
    expect(await screen.findByText('Demo catalog')).toBeInTheDocument();
    expect(screen.getByText(/quality scan 5\/10 frames/i)).toBeInTheDocument();
  });
});
