import { describe, expect, it } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { apiClient } from '../client';
import type { StackStretchPendingProgress } from '../types';

const jobId = 'a'.repeat(64);

const ready = {
  success: true,
  data: {
    schema_version: 2,
    stretch_id: 'b'.repeat(64),
    stretch_version: '0.1.0',
    deconvolution_version: null,
    deconvolution_id: null,
    config: { model: { type: 'identity' }, color_strategy: 'linked', max_analysis_samples: 1 },
    resolved_plan: {},
    source_transfer: 'linear',
    input_range: null,
    linked_statistics: { min: 0, max: 1, median: 0.5, mad: 0.1, count: 4 },
    channel_statistics: [],
    luminance_statistics: null,
    deconvolution: null,
    preview_url: '/p',
    original_preview_url: '/o',
    fits_url: null,
  },
  error: null,
};

describe('applyStackStretch polling', () => {
  it('polls 202 answers, reporting their progress, until the result is ready', async () => {
    let calls = 0;
    server.use(
      http.post(`/api/db/test/stack-previews/${jobId}/0/stretch`, () => {
        calls += 1;
        if (calls < 3) {
          return HttpResponse.json(
            {
              success: true,
              data: {
                pending: true,
                stage: 'RC-Astro StarXTerminator',
                fraction: calls * 0.25,
              },
              error: null,
            },
            { status: 202 }
          );
        }
        return HttpResponse.json(ready);
      })
    );

    const seen: StackStretchPendingProgress[] = [];
    const preview = await apiClient.applyStackStretch(
      'test',
      jobId,
      0,
      { model: { type: 'identity' }, color_strategy: 'linked' },
      { onProgress: (progress) => seen.push(progress), pollIntervalMs: 5 }
    );

    expect(calls).toBe(3);
    expect(seen.map((progress) => progress.fraction)).toEqual([0.25, 0.5]);
    expect(seen[0].stage).toBe('RC-Astro StarXTerminator');
    expect(preview.stretch_id).toBe('b'.repeat(64));
  });

  it('surfaces a failed poll as an error', async () => {
    server.use(
      http.post(`/api/db/test/stack-previews/${jobId}/0/stretch`, () =>
        HttpResponse.json(
          { success: false, data: null, error: 'sxt: not licensed' },
          { status: 400 }
        )
      )
    );
    await expect(
      apiClient.applyStackStretch(
        'test',
        jobId,
        0,
        { model: { type: 'identity' }, color_strategy: 'linked' },
        { pollIntervalMs: 5 }
      )
    ).rejects.toThrow(/not licensed/);
  });
});
