import { describe, expect, it } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackFrameDecision, StackMethod } from '../../api/types';
import { OPEN_SETTINGS_EVENT, settingsIntentOf } from '../../utils/settingsIntent';
import StackPreviewPanel from '../StackPreviewPanel';

const recommended: StackMethod = {
  normalization: 'local_background',
  weighting: 'noise',
  registration: 'quadratic',
  interpolation: 'lanczos3',
  bayer_drizzle: false,
  reference: 'auto',
  final_pass: 'reintegrate',
};

const classic: StackMethod = {
  normalization: 'global',
  weighting: 'equal',
  registration: 'similarity',
  interpolation: 'bilinear',
  bayer_drizzle: false,
  reference: 'best_graded',
  final_pass: 'reintegrate',
};

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

function decision(
  imageId: number,
  disposition: StackFrameDecision['disposition'],
  weight?: number[],
  noise?: number[]
): StackFrameDecision {
  return {
    image_id: imageId,
    disposition,
    reason: null,
    quality_score: 0.9,
    matched_stars: disposition === 'reference' ? null : 120,
    registration_rms_pixels: disposition === 'reference' ? null : 0.2,
    registration_drift_pixels: null,
    registered_mapping: null,
    normalization_mean_gain: 1,
    normalization_mean_offset: 0,
    source_fingerprint: null,
    overlap_fraction: 1,
    integrated_fraction: 1,
    ...(weight ? { integration_weight: weight } : {}),
    ...(noise ? { noise_sigma: noise } : {}),
  };
}

function mockLatest(built: StackMethod | undefined, frames: StackFrameDecision[] = [], current: StackMethod = recommended) {
  let submitted: Record<string, unknown> | undefined;
  server.use(
    http.get('/api/settings/stacking/method', () => ok({ method: current, recommended, classic })),
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
        ...(built ? { method: built } : {}),
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
          frames,
        },
      }],
    })),
    http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({
      schema_version: 1,
      database_id: 'test',
      project_id: 1,
      targets: [],
      jobs: [],
    })),
    http.post('/api/db/:dbId/projects/:projectId/stack-previews', async ({ request }) => {
      submitted = (await request.json()) as Record<string, unknown>;
      return HttpResponse.json(
        { success: false, data: null, error: 'not in this test', status: 'error' },
        { status: 500 }
      );
    })
  );
  return () => submitted;
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

describe('StackPreviewPanel stacking method', () => {
  it('names the server method, leaves it to the server, and keeps a matching build current', async () => {
    const submitted = mockLatest(recommended);
    const view = renderPanel();
    expect(await screen.findByRole('button', { name: 'Recommended' })).toBeInTheDocument();
    expect(screen.queryByRole('checkbox', { name: /weight frames/i })).toBeNull();
    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-card'))
        .toHaveAttribute('data-outdated', 'false')
    );

    await userEvent.click(screen.getByRole('button', { name: 'Rebuild current set' }));
    await waitFor(() => expect(submitted()).toBeDefined());
    expect(submitted()).not.toHaveProperty('method');
    expect(submitted()).not.toHaveProperty('weighting');
  });

  it('marks a build made with another method out of date', async () => {
    mockLatest(classic);
    const view = renderPanel();
    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-outdated'))
        .toHaveTextContent('Out of date — stacking method changed')
    );
  });

  it('calls a recommended method without the final pass a draft', async () => {
    const draft = { ...recommended, final_pass: 'draft' as const };
    mockLatest(draft, [], draft);
    renderPanel();
    expect(await screen.findByRole('button', { name: 'Draft' })).toBeInTheDocument();
  });

  it('opens the Stacking settings from the method name', async () => {
    mockLatest(recommended);
    renderPanel();
    const intents: Array<string | null> = [];
    const listen = (event: Event) => intents.push(settingsIntentOf(event));
    window.addEventListener(OPEN_SETTINGS_EVENT, listen);
    try {
      await userEvent.click(await screen.findByRole('button', { name: 'Recommended' }));
    } finally {
      window.removeEventListener(OPEN_SETTINGS_EVENT, listen);
    }
    expect(intents).toEqual(['stacking']);
  });

  it('shows each frame weight for a noise-weighted build', async () => {
    mockLatest(recommended, [
      decision(1, 'reference', [1], [2.0]),
      decision(2, 'accepted', [0.25], [4.0]),
    ]);
    const view = renderPanel();
    await waitFor(() =>
      expect(view.container.querySelector('.stack-frame-table-wrap table')).not.toBeNull()
    );
    const table = view.container.querySelector('.stack-frame-table-wrap table') as HTMLElement;
    expect(within(table).getByRole('columnheader', { name: 'Weight' })).toBeInTheDocument();
    const weights = [...table.querySelectorAll('.stack-frame-weight')].map(
      (cell) => cell.textContent
    );
    expect(weights).toEqual(['1.00', '0.25']);
    expect(table.querySelectorAll('.stack-frame-weight')[1]).toHaveAttribute('title', 'Noise 4.00');
  });

  it('leaves the weight column out of an equally weighted build', async () => {
    mockLatest(classic, [decision(1, 'reference'), decision(2, 'accepted')], classic);
    const view = renderPanel();
    await waitFor(() =>
      expect(view.container.querySelector('.stack-frame-table-wrap table')).not.toBeNull()
    );
    const table = view.container.querySelector('.stack-frame-table-wrap table') as HTMLElement;
    expect(within(table).queryByRole('columnheader', { name: 'Weight' })).toBeNull();
  });

  it('marks a stack from before the method out of date, keeping its frame weights', async () => {
    mockLatest(undefined, [
      decision(1, 'reference', [1], [2.0]),
      decision(2, 'accepted', [0.5], [3.0]),
    ]);
    const view = renderPanel();
    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-outdated'))
        .toHaveTextContent('Out of date — built before the stacking method could be chosen')
    );
    const table = view.container.querySelector('.stack-frame-table-wrap table') as HTMLElement;
    expect(within(table).getByRole('columnheader', { name: 'Weight' })).toBeInTheDocument();
  });
});
