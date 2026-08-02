import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
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

const runningGroup = {
  index: 0,
  target_id: 42,
  target_name: 'Sh2 86',
  filter_name: 'Ha',
  state: 'running',
  phase: 'stacking',
  total_candidates: 2,
  eligible_frames: 2,
  quality_excluded: 0,
  missing_files: 0,
  processed_frames: 1,
  accepted_frames: 1,
  rejected_frames: 0,
  output_channels: 1,
  reference_image_id: 1,
  total_exposure_seconds: 120,
  preview_url: null,
  fits_url: null,
  error: null,
  calibration: { state: 'none', bias_frames: 0, dark_frames: 0, dark_flat_frames: 0, flat_frames: 0, warning: null },
  input_images: [
    { image_id: 1, grading_status: 1 },
    { image_id: 2, grading_status: 1 },
  ],
  frames: [],
};

describe('StackPreviewPanel job adoption', () => {
  it('shows a build that was started before the panel mounted', async () => {
    server.use(
      http.get('/api/stack-activity', () => ok({
        schema_version: 1,
        active: [{
          kind: 'mono',
          job_id: 'job-a',
          database_id: 'test',
          project_id: 1,
          state: 'running',
          label: 'Sh2 86 · Ha',
          detail: 'Registering frames',
          processed_units: 1,
          total_units: 2,
          created_unix_seconds: 100,
        }],
      })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/latest', () => ok({
        schema_version: 1,
        database_id: 'test',
        project_id: 1,
        updated_unix_seconds: 0,
        groups: [],
      })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({
        schema_version: 1,
        database_id: 'test',
        project_id: 1,
        targets: [],
        jobs: [],
      })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/job-a', () => ok({
        schema_version: 2,
        job_id: 'job-a',
        database_id: 'test',
        project_id: 1,
        state: 'running',
        accepted_only: false,
        created_unix_seconds: 100,
        artifact_revision: 'rev-a',
        cache_version: 7,
        stacking_version: '0.2.0',
        groups: [runningGroup],
        error: null,
      })),
    );

    render(
      <StackPreviewPanel
        dbId="test"
        projectId={1}
        images={images}
        selectionSource="visible"
        onOpenImage={() => undefined}
      />,
      { wrapper: wrapper() }
    );

    expect(await screen.findAllByText('Registering frames')).not.toHaveLength(0);
    expect(await screen.findByText('1/2 frames')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: /Sh2 86 Ha stack progress/i }))
      .toHaveAttribute('aria-valuenow', '1');
    expect(screen.getByRole('button', { name: 'Building previews…' })).toBeDisabled();
  });
});
