import { describe, it, expect } from 'vitest';
import { renderHook, waitFor, act } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { useSpatialScan } from '../useSpatialScan';

function scanStatus(overrides: {
  started?: boolean;
  running?: boolean;
  total?: number;
  processed?: number;
  cached_count?: number;
}) {
  return {
    success: true,
    data: {
      started: overrides.started ?? false,
      progress: {
        running: overrides.running ?? false,
        target_id: 1,
        filter_name: null,
        total: overrides.total ?? 0,
        processed: overrides.processed ?? 0,
        skipped_cached: 0,
        errors: 0,
        current_file: null,
        started_at: null,
        finished_at: null,
        last_error: null,
      },
      cached_count: overrides.cached_count ?? 0,
    },
    error: null,
    status: 'ready',
  };
}

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        gcTime: 0,
      },
    },
  });
  return {
    queryClient,
    wrapper: function Wrapper({ children }: { children: ReactNode }) {
      return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
    },
  };
}

describe('useSpatialScan', () => {
  it('reports idle status by default', async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useSpatialScan('test', 1), { wrapper });

    await waitFor(() => {
      expect(result.current.status).toBeDefined();
    });
    expect(result.current.isRunning).toBe(false);
    expect(result.current.status?.cached_count).toBe(0);
  });

  it('start() posts the scan request and seeds running progress', async () => {
    let postedBody: unknown = null;
    server.use(
      http.post('/api/db/:dbId/analysis/quality-scan', async ({ request }) => {
        postedBody = await request.json();
        return HttpResponse.json(
          scanStatus({ started: true, running: true, total: 12, processed: 0 })
        );
      }),
      http.get('/api/db/:dbId/analysis/quality-scan', () =>
        HttpResponse.json(scanStatus({ running: true, total: 12, processed: 3 }))
      )
    );

    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useSpatialScan('test', 42, 'R'), { wrapper });

    act(() => {
      result.current.start(undefined);
    });

    await waitFor(() => {
      expect(result.current.isRunning).toBe(true);
    });
    expect(postedBody).toMatchObject({ target_id: 42, filter_name: 'R' });
    expect(result.current.status?.progress.total).toBe(12);
  });

  it('invalidates sequence analysis when a scan finishes', async () => {
    // First poll: running. Later polls: finished.
    let polls = 0;
    server.use(
      http.get('/api/db/:dbId/analysis/quality-scan', () => {
        polls += 1;
        return HttpResponse.json(
          scanStatus({ running: polls < 2, total: 5, processed: polls < 2 ? 2 : 5 })
        );
      })
    );

    const { queryClient, wrapper } = createWrapper();
    const invalidated: unknown[] = [];
    const original = queryClient.invalidateQueries.bind(queryClient);
    queryClient.invalidateQueries = ((filters?: { queryKey?: unknown }) => {
      invalidated.push(filters?.queryKey);
      return original(filters as never);
    }) as typeof queryClient.invalidateQueries;

    const { result } = renderHook(() => useSpatialScan('test', 1), { wrapper });

    await waitFor(
      () => {
        expect(result.current.isRunning).toBe(false);
        expect(result.current.status?.progress.processed).toBe(5);
      },
      { timeout: 5000 }
    );

    await waitFor(() => {
      expect(
        invalidated.some(
          (key) => Array.isArray(key) && key.includes('sequence-analysis')
        )
      ).toBe(true);
    });
  });
});
