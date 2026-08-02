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
  scope?: {
    target_id: number;
    filter_name: string | null;
    total_frames: number;
    pending_frames: number;
    new_frames: number;
    outdated_frames: number;
    needs_analysis: boolean;
  };
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
      ...(overrides.scope ? { scope: overrides.scope } : {}),
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

  it('reports whether the selected target has quality work', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/quality-scan', ({ request }) => {
        const params = new URL(request.url).searchParams;
        if (!params.has('target_id')) {
          return HttpResponse.json(scanStatus({}));
        }
        return HttpResponse.json(scanStatus({
          scope: {
            target_id: 42,
            filter_name: 'R',
            total_frames: 5,
            pending_frames: 2,
            new_frames: 1,
            outdated_frames: 1,
            needs_analysis: true,
          },
        }));
      }),
    );

    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useSpatialScan('test', 42, 'R'), { wrapper });

    await waitFor(() => {
      expect(result.current.scope?.pending_frames).toBe(2);
    });
    expect(result.current.scope?.needs_analysis).toBe(true);
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
      http.get('/api/db/:dbId/analysis/quality-scan', ({ request }) => {
        if (new URL(request.url).searchParams.has('target_id')) {
          return HttpResponse.json(scanStatus({
            scope: {
              target_id: 42,
              filter_name: 'R',
              total_frames: 12,
              pending_frames: 12,
              new_frames: 12,
              outdated_frames: 0,
              needs_analysis: true,
            },
          }));
        }
        return HttpResponse.json(scanStatus({ running: true, total: 12, processed: 3 }));
      })
    );

    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useSpatialScan('test', 42, 'R'), { wrapper });

    act(() => {
      result.current.start(true);
    });

    await waitFor(() => {
      expect(result.current.isRunning).toBe(true);
    });
    expect(postedBody).toMatchObject({ target_id: 42, filter_name: 'R', force: true });
    expect(result.current.status?.progress.total).toBe(12);
  });

  it('invalidates sequence analysis when a scan finishes', async () => {
    // First poll: running. Later polls: finished.
    let polls = 0;
    server.use(
      http.get('/api/db/:dbId/analysis/quality-scan', ({ request }) => {
        if (new URL(request.url).searchParams.has('target_id')) {
          return HttpResponse.json(scanStatus({
            scope: {
              target_id: 1,
              filter_name: null,
              total_frames: 5,
              pending_frames: 0,
              new_frames: 0,
              outdated_frames: 0,
              needs_analysis: false,
            },
          }));
        }
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
      expect(
        invalidated.some(
          (key) => Array.isArray(key) && key.includes('quality-scan-scope')
        )
      ).toBe(true);
    });
  });
});
