import { describe, it, expect } from 'vitest';
import { renderHook, waitFor, act } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { useScopedQuality, useSequenceAnalysis, useImageQuality } from '../useSequenceAnalysis';
import normalFixture from '../../__fixtures__/sequence-analysis-normal.json';
import imageQualityFixture from '../../__fixtures__/image-quality-context.json';

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        gcTime: 0,
      },
    },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        {children}
      </QueryClientProvider>
    );
  };
}

describe('useSequenceAnalysis', () => {
  it('starts with no data and not loading', () => {
    const { result } = renderHook(() => useSequenceAnalysis('test'), {
      wrapper: createWrapper(),
    });

    expect(result.current.data).toBeUndefined();
    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it('fetches data after analyze() is called', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(normalFixture);
      }),
    );

    const { result } = renderHook(() => useSequenceAnalysis('test'), {
      wrapper: createWrapper(),
    });

    act(() => {
      result.current.analyze({ target_id: 1, filter_name: 'L' });
    });

    await waitFor(() => {
      expect(result.current.data).toBeDefined();
    });

    expect(result.current.data!.sequences).toHaveLength(1);
    expect(result.current.data!.sequences[0].target_name).toBe('M42');
    expect(result.current.isLoading).toBe(false);
  });

  it('resets state when reset() is called', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(normalFixture);
      }),
    );

    const { result } = renderHook(() => useSequenceAnalysis('test'), {
      wrapper: createWrapper(),
    });

    // Trigger analysis
    act(() => {
      result.current.analyze({ target_id: 1 });
    });

    await waitFor(() => {
      expect(result.current.data).toBeDefined();
    });

    // Reset
    act(() => {
      result.current.reset();
    });

    // After reset, query is disabled (no target_id), so data becomes undefined
    await waitFor(() => {
      expect(result.current.isLoading).toBe(false);
    });
  });

  it('handles server errors', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(
          { success: false, data: null, error: 'Target not found', status: null },
          { status: 400 },
        );
      }),
    );

    const { result } = renderHook(() => useSequenceAnalysis('test'), {
      wrapper: createWrapper(),
    });

    act(() => {
      result.current.analyze({ target_id: 9999 });
    });

    await waitFor(() => {
      expect(result.current.error).not.toBeNull();
    });
  });
});

describe('useImageQuality', () => {
  it('does not fetch when imageId is undefined', () => {
    const { result } = renderHook(() => useImageQuality('test', undefined), {
      wrapper: createWrapper(),
    });

    expect(result.current.data).toBeUndefined();
    expect(result.current.isLoading).toBe(false);
  });

  it('fetches image quality when imageId is provided', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/image/:imageId', () => {
        return HttpResponse.json(imageQualityFixture);
      }),
    );

    const { result } = renderHook(() => useImageQuality('test', 5), {
      wrapper: createWrapper(),
    });

    await waitFor(() => {
      expect(result.current.data).toBeDefined();
    });

    expect(result.current.data!.image_id).toBe(5);
    expect(result.current.data!.quality).toBeDefined();
    expect(result.current.data!.quality!.quality_score).toBe(0.70);
    expect(result.current.data!.sequence_filter_name).toBe('L');
  });
});

describe('useScopedQuality', () => {
  it('loads one project analysis and indexes results by image', async () => {
    let requests = 0;
    let projectId: string | null = null;
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', ({ request }) => {
        requests += 1;
        projectId = new URL(request.url).searchParams.get('project_id');
        return HttpResponse.json(normalFixture);
      }),
    );

    const { result } = renderHook(() => useScopedQuality('test', 7, undefined), {
      wrapper: createWrapper(),
    });

    await waitFor(() => {
      expect(result.current.qualityByImage.size).toBe(10);
    });

    expect(requests).toBe(1);
    expect(projectId).toBe('7');
    expect(result.current.qualityByImage.get(1)?.details).toBe(
      normalFixture.data.sequences[0].images[0].details,
    );
  });

  it('keeps All Projects unscored without starting a database-wide analysis', () => {
    let requests = 0;
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        requests += 1;
        return HttpResponse.json(normalFixture);
      }),
    );

    const { result } = renderHook(() => useScopedQuality('test', null, null), {
      wrapper: createWrapper(),
    });

    expect(result.current.isScoped).toBe(false);
    expect(result.current.qualityByImage.size).toBe(0);
    expect(requests).toBe(0);
  });
});
