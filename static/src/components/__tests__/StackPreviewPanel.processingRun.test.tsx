import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackActivityEntry, StackStretchPreview } from '../../api/types';
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
    return <QueryClientProvider client={queryClient}><MemoryRouter>{children}</MemoryRouter></QueryClientProvider>;
  };
}

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function preview(stretchId: string): StackStretchPreview {
  return {
    schema_version: 2,
    stretch_id: stretchId,
    stretch_version: '0.1.0',
    deconvolution_version: null,
    deconvolution_id: null,
    config: { model: { type: 'auto-mtf', target_median: 0.2, shadows_clip: -2.8 }, color_strategy: 'linked', max_analysis_samples: 1 },
    request: {
      model: { type: 'auto-mtf', target_median: 0.2, shadows_clip: -2.8 },
      color_strategy: 'linked',
      deconvolution: null,
      rc_astro: null,
    },
    resolved_plan: {},
    source_transfer: 'linear',
    input_range: null,
    linked_statistics: { min: 0, max: 1, median: 0.2, mad: 0.1, count: 4 },
    channel_statistics: [],
    luminance_statistics: null,
    deconvolution: null,
    preview_url: `/api/db/test/stack-previews/stretch/${stretchId}/preview`,
    original_preview_url: `/api/db/test/stack-previews/stretch/${stretchId}/preview?size=original`,
    fits_url: null,
  };
}

/** A BlurXTerminator run a person asked for on group `groupIndex` of the stack. */
function run(groupIndex: number, automatic = false): StackActivityEntry {
  return {
    kind: 'processing', job_id: 'r'.repeat(64), database_id: 'test', project_id: 1, target_id: 42,
    state: 'running', label: 'Sh2 86 · Ha', detail: 'BlurXTerminator', automatic,
    processed_units: 420, total_units: 1000, created_unix_seconds: 1,
    progress_label: 'BlurXTerminator · 42%',
    processing: {
      operation: 'BlurXTerminator', channel: 'Ha', stoppable: true,
      stack_job_id: 'stack-job', group_index: groupIndex,
      stage: 'BlurXTerminator', fraction: 0.42,
    },
  };
}

/** One ready mono card, whose processing the page did not ask for itself. */
function mockCard(
  stretch: () => StackStretchPreview | null,
  active: () => StackActivityEntry[],
  reads: { count: number },
) {
  server.use(
    http.get('/api/settings/stacking/method', () => ok({ method: {}, recommended: {}, classic: {} })),
    http.get('/api/stack-activity', () => ok({ schema_version: 1, active: active() })),
    http.get('/api/db/:dbId/projects/:projectId/stack-previews/latest', () => ok({
      schema_version: 2,
      database_id: 'test',
      project_id: 1,
      updated_unix_seconds: 100,
      groups: [{
        job_id: 'stack-job',
        artifact_revision: 'revision-1',
        accepted_only: false,
        created_unix_seconds: 90,
        cache_version: 13,
        order: 'capture',
        group: {
          index: 0, target_id: 42, target_name: 'Sh2 86', filter_name: 'Ha', state: 'ready',
          phase: 'ready', total_candidates: 2, eligible_frames: 2, quality_excluded: 0,
          missing_files: 0, processed_frames: 2, accepted_frames: 2, rejected_frames: 0,
          output_channels: 1, reference_image_id: 1, total_exposure_seconds: 120,
          preview_url: null, fits_url: null, error: null,
          calibration: { state: 'none', bias_frames: 0, dark_frames: 0, dark_flat_frames: 0, flat_frames: 0, warning: null },
          input_images: [{ image_id: 1, grading_status: 1 }, { image_id: 2, grading_status: 1 }],
          frames: [],
        },
      }],
    })),
    http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({
      schema_version: 1, database_id: 'test', project_id: 1, targets: [], jobs: [],
    })),
    http.get('/api/db/test/stack-previews/stack-job/0/stretch', () => {
      reads.count += 1;
      return ok(stretch());
    }),
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

describe('StackPreviewPanel with a processing run asked for before the page opened', () => {
  it('shows the run on its card, then reads the result when it ends', async () => {
    let active: StackActivityEntry[] = [run(0)];
    let stretch: StackStretchPreview | null = null;
    const reads = { count: 0 };
    mockCard(() => stretch, () => active, reads);
    renderPanel();

    const applying = await screen.findByRole('button', { name: 'Applying… 42% · BlurXTerminator' });
    expect(applying).toBeDisabled();
    expect(applying).toHaveAttribute('aria-busy', 'true');
    expect(screen.getByRole('button', { name: 'Revert processing' })).toBeDisabled();
    await waitFor(() => expect(reads.count).toBeGreaterThan(0));
    const readsWhileRunning = reads.count;

    // The run ends on the server with its result saved.
    stretch = preview('blurx-done');
    active = [];
    const apply = await screen.findByRole('button', { name: 'Apply processing' }, { timeout: 5000 });
    expect(apply).toBeEnabled();
    await waitFor(() => expect(reads.count).toBeGreaterThan(readsWhileRunning));
    await waitFor(() => expect(screen.getByAltText('Sh2 86 Ha stack preview'))
      .toHaveAttribute('src', '/api/db/test/stack-previews/stretch/blurx-done/preview'));
  });

  it('leaves the card alone for a run on another group or one the server re-applies', async () => {
    const reads = { count: 0 };
    mockCard(() => null, () => [run(1), run(0, true)], reads);
    renderPanel();

    const apply = await screen.findByRole('button', { name: 'Apply processing' });
    await waitFor(() => expect(reads.count).toBeGreaterThan(0));
    expect(apply).toBeEnabled();
    expect(screen.queryByRole('button', { name: /Applying/ })).toBeNull();
  });
});
