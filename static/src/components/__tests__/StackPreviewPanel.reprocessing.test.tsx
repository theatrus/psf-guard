import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackActivityEntry, StackReprocessing, StackStretchPreview } from '../../api/types';
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

function preview(stretchId: string, reprocessing?: StackReprocessing): StackStretchPreview {
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
    ...(reprocessing ? { reprocessing } : {}),
  };
}

const run: StackActivityEntry = {
  kind: 'processing', job_id: 'r'.repeat(64), database_id: 'test', project_id: 1, target_id: 42,
  state: 'running', label: 'Sh2 86 · Ha', detail: 'BlurXTerminator', automatic: true,
  processed_units: 420, total_units: 1000, created_unix_seconds: 1,
  progress_label: 'BlurXTerminator · 42%',
  processing: { operation: 'BlurXTerminator', channel: 'Ha', stoppable: true, stage: 'BlurXTerminator', fraction: 0.42 },
};

/** A card on a refreshed stack whose processing the server is making again. */
function mockCard(
  stretch: () => StackStretchPreview | null,
  active: () => StackActivityEntry[],
  group: Record<string, unknown> = {},
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
        job_id: 'refreshed-job',
        artifact_revision: 'revision-2',
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
          ...group,
        },
      }],
    })),
    http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({
      schema_version: 1, database_id: 'test', project_id: 1, targets: [], jobs: [],
    })),
    http.get('/api/db/test/stack-previews/refreshed-job/0/stretch', () => ok(stretch())),
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

describe('StackPreviewPanel on a refreshed stack', () => {
  it('says the preview render is queued while the stack stays ready', async () => {
    const group = {
      index: 0, target_id: 42, target_name: 'Sh2 86', filter_name: 'Ha', state: 'ready',
      phase: 'render-queued', total_candidates: 2, eligible_frames: 2, quality_excluded: 0,
      missing_files: 0, processed_frames: 2, accepted_frames: 2, rejected_frames: 0,
      output_channels: 1, reference_image_id: 1, total_exposure_seconds: 120,
      preview_url: null, fits_url: null, error: null,
      calibration: { state: 'none', bias_frames: 0, dark_frames: 0, dark_flat_frames: 0, flat_frames: 0, warning: null },
      input_images: [{ image_id: 1, grading_status: 1 }, { image_id: 2, grading_status: 1 }],
      frames: [],
    };
    const rendering: StackActivityEntry = {
      kind: 'mono', job_id: 'refreshed-job', database_id: 'test', project_id: 1, target_id: 42,
      state: 'queued', label: 'Sh2 86 · Ha', detail: 'Waiting to render preview', automatic: false,
      processed_units: 2, total_units: 2, created_unix_seconds: 1,
    };
    mockCard(() => null, () => [rendering]);
    server.use(http.get('/api/db/test/projects/1/stack-previews/refreshed-job', () => ok({
      schema_version: 2, job_id: 'refreshed-job', database_id: 'test', project_id: 1, state: 'queued',
      accepted_only: false, created_unix_seconds: 90, artifact_revision: 'revision-2', cache_version: 13,
      stacking_version: '0.40.0', order: 'capture', groups: [group], error: null,
    })));
    renderPanel();
    expect(await screen.findByText('Preview render queued')).toBeInTheDocument();
  });

  it('loads a preview rendered again from the FITS under its own revision', async () => {
    mockCard(() => null, () => [], { preview_revision: 'rendered-3' });
    const view = renderPanel();
    await waitFor(() => expect(view.container.querySelector('.stack-preview-image img')).not.toBeNull());
    expect(view.container.querySelector('.stack-preview-image img')).toHaveAttribute(
      'src',
      expect.stringContaining('/stack-previews/refreshed-job/0/preview?v=revision-2.rendered-3'),
    );
  });

  it('shows the previous result while reprocessing, with the run from the queue, then the new one', async () => {
    let done = false;
    mockCard(
      () => done
        ? preview('n'.repeat(64))
        : preview('o'.repeat(64), { state: 'running', from_revision: 'revision-1', run_id: 'r'.repeat(64) }),
      () => (done ? [] : [run]),
    );
    const view = renderPanel();
    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-carried'))
        .toHaveTextContent('From the previous stack · reprocessing · BlurXTerminator · 42%')
    );
    const image = view.container.querySelector('.stack-preview-image img');
    expect(image).toHaveAttribute('src', expect.stringContaining('o'.repeat(64)));

    // The re-apply lands: the card swaps in its own result.
    done = true;
    await waitFor(() => expect(view.container.querySelector('.stack-preview-carried')).toBeNull(), {
      timeout: 6000,
    });
    expect(view.container.querySelector('.stack-preview-image img'))
      .toHaveAttribute('src', expect.stringContaining('n'.repeat(64)));
  }, 12_000);

  it('keeps the previous result and says why reprocessing failed', async () => {
    mockCard(
      () => preview('o'.repeat(64), {
        state: 'failed', from_revision: 'revision-1', error: 'BlurXTerminator is not licensed on this server',
      }),
      () => [],
    );
    renderPanel();
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(
      'From the previous stack · reprocessing failed: BlurXTerminator is not licensed on this server'
    );
  });
});
