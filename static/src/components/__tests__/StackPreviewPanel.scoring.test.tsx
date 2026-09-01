import { act, render, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import {
  SCORING_DEFAULTS,
  setScoringPreferences,
} from '../../hooks/useScoringPreferences';
import type { StackScoringSettings } from '../../api/types';
import StackPreviewPanel from '../StackPreviewPanel';

const images = [
  { id: 1, target_id: 42, target_name: 'Sh2 86', filter_name: 'Ha', grading_status: 1 },
  { id: 2, target_id: 42, target_name: 'Sh2 86', filter_name: 'Ha', grading_status: 1 },
];

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function mockLatest(scoring?: StackScoringSettings) {
  server.use(
    http.get('/api/stack-activity', () => ok({ schema_version: 1, active: [] })),
    http.get('/api/db/:dbId/projects/:projectId/stack-previews/latest', () => ok({
      schema_version: 2,
      database_id: 'test',
      project_id: 1,
      updated_unix_seconds: 100,
      groups: [{
        job_id: 'completed-job',
        artifact_revision: 'completed-revision',
        accepted_only: false,
        created_unix_seconds: 90,
        cache_version: 13,
        order: 'capture',
        ...(scoring ? { scoring } : {}),
        group: {
          index: 0,
          target_id: 42,
          target_name: 'Sh2 86',
          filter_name: 'Ha',
          state: 'ready',
          phase: 'ready',
          total_candidates: 2,
          eligible_frames: 2,
          quality_excluded: 0,
          missing_files: 0,
          processed_frames: 2,
          accepted_frames: 2,
          rejected_frames: 0,
          output_channels: 1,
          reference_image_id: 1,
          total_exposure_seconds: 120,
          preview_url: null,
          fits_url: null,
          error: null,
          calibration: {
            state: 'none',
            bias_frames: 0,
            dark_frames: 0,
            dark_flat_frames: 0,
            flat_frames: 0,
            warning: null,
          },
          input_images: [
            { image_id: 1, grading_status: 1 },
            { image_id: 2, grading_status: 1 },
          ],
          frames: [],
        },
      }],
    })),
    http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({
      schema_version: 1,
      database_id: 'test',
      project_id: 1,
      targets: [],
      jobs: [],
    }))
  );
}

function renderPanel() {
  return render(
    <StackPreviewPanel
      dbId="test"
      projectId={1}
      images={images}
      selectionSource="visible"
      onOpenImage={() => undefined}
    />,
    { wrapper: wrapper() }
  );
}

beforeEach(() => setScoringPreferences({ ...SCORING_DEFAULTS }));
afterEach(() => setScoringPreferences({ ...SCORING_DEFAULTS }));

describe('StackPreviewPanel scoring provenance', () => {
  it('treats an old artifact without scoring metadata as calibrated', async () => {
    mockLatest();
    const view = renderPanel();

    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-card'))
        .toHaveAttribute('data-outdated', 'false')
    );
  });

  it('marks a remembered artifact stale when scoring settings change', async () => {
    mockLatest({
      penalty_satellite: 1,
      penalty_pointing: 1,
      penalty_temporal: 1,
      hfr_reject_above: null,
      star_count_reject_below: null,
    });
    const view = renderPanel();
    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-card'))
        .toHaveAttribute('data-outdated', 'false')
    );

    act(() => {
      setScoringPreferences({ ...SCORING_DEFAULTS, satellite: 0 });
    });

    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-outdated'))
        .toHaveTextContent('Out of date — scoring settings changed')
    );
  });
});
