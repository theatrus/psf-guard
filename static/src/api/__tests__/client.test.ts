import { describe, it, expect, beforeEach } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { apiClient } from '../client';
import normalFixture from '../../__fixtures__/sequence-analysis-normal.json';
import cloudsFixture from '../../__fixtures__/sequence-analysis-clouds.json';
import imageQualityFixture from '../../__fixtures__/image-quality-context.json';

// Reset the API client's lazy initialization between tests
// by clearing the cached instance so each test starts fresh
beforeEach(() => {
  // The apiClient module caches its axios instance; since we're using
  // MSW to intercept at the network level, this works fine as-is.
});

describe('apiClient.analyzeSequence', () => {
  it('returns sequence analysis data for a target', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(normalFixture);
      }),
    );

    const result = await apiClient.analyzeSequence({ target_id: 1, filter_name: 'L' });

    expect(result.sequences).toHaveLength(1);
    expect(result.sequences[0].target_id).toBe(1);
    expect(result.sequences[0].target_name).toBe('M42');
    expect(result.sequences[0].filter_name).toBe('L');
    expect(result.sequences[0].image_count).toBe(10);
    expect(result.sequences[0].images).toHaveLength(10);
  });

  it('returns cloud-affected sequence data', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
    );

    const result = await apiClient.analyzeSequence({ target_id: 1, filter_name: 'Ha' });

    expect(result.sequences).toHaveLength(1);
    expect(result.sequences[0].image_count).toBe(8);
    expect(result.sequences[0].summary.cloud_events_detected).toBe(1);

    // Cloud frames should have low scores
    const cloudImages = result.sequences[0].images.filter(img => img.category === 'likely_clouds');
    expect(cloudImages).toHaveLength(2);
    cloudImages.forEach(img => {
      expect(img.quality_score).toBeLessThan(0.3);
    });
  });

  it('sends query parameters correctly', async () => {
    let capturedUrl: URL | null = null;

    server.use(
      http.get('/api/analysis/sequence', ({ request }) => {
        capturedUrl = new URL(request.url);
        return HttpResponse.json(normalFixture);
      }),
    );

    await apiClient.analyzeSequence({
      target_id: 1,
      filter_name: 'Ha',
      session_gap_minutes: 90,
      weight_star_count: 0.5,
    });

    expect(capturedUrl).not.toBeNull();
    expect(capturedUrl!.searchParams.get('target_id')).toBe('1');
    expect(capturedUrl!.searchParams.get('filter_name')).toBe('Ha');
    expect(capturedUrl!.searchParams.get('session_gap_minutes')).toBe('90');
    expect(capturedUrl!.searchParams.get('weight_star_count')).toBe('0.5');
  });

  it('throws when data is null', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json({
          success: true,
          data: null,
          error: null,
          status: 'ready',
        });
      }),
    );

    await expect(apiClient.analyzeSequence({ target_id: 1 })).rejects.toThrow('Sequence analysis failed');
  });
});

describe('apiClient.getImageQuality', () => {
  it('returns image quality context', async () => {
    server.use(
      http.get('/api/analysis/image/:imageId', () => {
        return HttpResponse.json(imageQualityFixture);
      }),
    );

    const result = await apiClient.getImageQuality(5);

    expect(result.image_id).toBe(5);
    expect(result.quality).toBeDefined();
    expect(result.quality!.quality_score).toBe(0.70);
    expect(result.sequence_target_id).toBe(1);
    expect(result.sequence_filter_name).toBe('L');
    expect(result.sequence_image_count).toBe(10);
    expect(result.reference_values).toBeDefined();
    expect(result.reference_values!.best_star_count).toBe(1.0);
  });

  it('throws when data is null', async () => {
    server.use(
      http.get('/api/analysis/image/:imageId', () => {
        return HttpResponse.json({
          success: false,
          data: null,
          error: 'Image not found',
          status: null,
        }, { status: 404 });
      }),
    );

    await expect(apiClient.getImageQuality(99999)).rejects.toThrow();
  });
});
