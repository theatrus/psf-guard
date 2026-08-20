import { describe, expect, it } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
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

const group = {
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
};

function job(state: string, groupState: string) {
  return {
    schema_version: 2,
    job_id: 'job-a',
    database_id: 'test',
    project_id: 1,
    state,
    accepted_only: false,
    created_unix_seconds: 100,
    artifact_revision: 'rev-a',
    cache_version: 7,
    stacking_version: '0.2.2',
    groups: [{ ...group, state: groupState, phase: groupState }],
    error: null,
  };
}

describe('StackPreviewPanel stop', () => {
  it('keeps a completed curve bound to its image during and after a stopped rebuild', async () => {
    const completedSnr = {
      order: 'capture',
      points: [
        { frames: 1, exposure_seconds: 120, noise: 20, background: 1000, signal: 400, snr: 20 },
        { frames: 2, exposure_seconds: 240, noise: 14, background: 1000, signal: 400, snr: 28.6 },
        { frames: 4, exposure_seconds: 480, noise: 10, background: 1000, signal: 400, snr: 40 },
      ],
      analysis: null,
    };
    const liveSnr = {
      order: 'capture',
      points: [
        { frames: 1, exposure_seconds: 120, noise: 30, background: 900, signal: 300, snr: 10 },
        { frames: 2, exposure_seconds: 240, noise: 25, background: 900, signal: 300, snr: 12 },
      ],
      analysis: null,
    };
    const completedGroup = {
      ...group,
      state: 'ready',
      phase: 'ready',
      processed_frames: 2,
      accepted_frames: 2,
      snr: completedSnr,
    };
    const rebuildingGroup = { ...group, snr: liveSnr };
    let currentJob = {
      ...job('running', 'running'),
      order: 'capture',
      groups: [rebuildingGroup],
    };

    server.use(
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
          cache_version: 7,
          order: 'capture',
          group: completedGroup,
        }],
      })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({
        schema_version: 1,
        database_id: 'test',
        project_id: 1,
        targets: [],
        jobs: [],
      })),
      http.post('/api/db/:dbId/projects/:projectId/stack-previews', () => ok(currentJob)),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/job-a', () => ok(currentJob)),
      http.post('/api/db/:dbId/projects/:projectId/stack-previews/job-a/cancel', () => {
        currentJob = {
          ...currentJob,
          state: 'cancelled',
          groups: [{ ...rebuildingGroup, state: 'cancelled', phase: 'cancelled' }],
        };
        return ok(currentJob);
      }),
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

    await userEvent.click(await screen.findByRole('button', { name: 'Rebuild current set' }));

    const completed = await screen.findByRole('group', {
      name: 'Sh2 86 Ha completed stack signal-to-noise analysis',
    });
    expect(within(completed).getByText('Exact measurements (3)')).toBeInTheDocument();

    const live = await screen.findByRole('region', { name: 'Live build SNR progress' });
    expect(within(live).getByText('Live rebuild progress')).toBeInTheDocument();
    expect(within(live).getByText('Exact measurements (2)')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Stop' }));
    expect(await screen.findByText('Stack stopped')).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.queryByRole('region', { name: 'Live build SNR progress' })).not.toBeInTheDocument()
    );
    expect(screen.getByRole('group', {
      name: 'Sh2 86 Ha completed stack signal-to-noise analysis',
    })).toBeInTheDocument();
  });

  it('marks a different order stale and resets quality order on navigation', async () => {
    const rememberedGroup = {
      ...group,
      state: 'ready',
      phase: 'ready',
      processed_frames: 2,
      accepted_frames: 2,
    };
    server.use(
      http.get(
        '/api/db/:dbId/projects/:projectId/stack-previews/latest',
        ({ params }) => ok({
          schema_version: 2,
          database_id: params.dbId,
          project_id: Number(params.projectId),
          updated_unix_seconds: 100,
          groups: params.projectId === '1'
            ? [{
                job_id: 'completed-job',
                artifact_revision: 'completed-revision',
                accepted_only: false,
                created_unix_seconds: 90,
                cache_version: 7,
                order: 'capture',
                group: rememberedGroup,
              }]
            : [],
        })
      ),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', ({ params }) => ok({
        schema_version: 1,
        database_id: params.dbId,
        project_id: Number(params.projectId),
        targets: [],
        jobs: [],
      })),
    );

    const view = render(
      <StackPreviewPanel
        dbId="test"
        projectId={1}
        images={images}
        selectionSource="visible"
        onOpenImage={() => undefined}
      />,
      { wrapper: wrapper() }
    );

    const order = await screen.findByLabelText('Order');
    await userEvent.selectOptions(order, 'quality');
    expect(await screen.findByText('Out of date — frame order changed')).toBeInTheDocument();

    view.rerender(
      <StackPreviewPanel
        dbId="other"
        projectId={2}
        images={images}
        selectionSource="visible"
        onOpenImage={() => undefined}
      />
    );

    await waitFor(() => expect(screen.getByLabelText('Order')).toHaveValue('capture'));
    await waitFor(() =>
      expect(screen.queryByText('Out of date — frame order changed')).not.toBeInTheDocument()
    );
  });

  it('stops a running build and reports the channel as stopped', async () => {
    let cancelled = false;
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/latest', () => ok({
        schema_version: 2,
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
      http.post('/api/db/:dbId/projects/:projectId/stack-previews', () =>
        ok(job('running', 'running'))
      ),
      http.post('/api/db/:dbId/projects/:projectId/stack-previews/job-a/cancel', () => {
        cancelled = true;
        return ok(job('cancelled', 'cancelled'));
      }),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/job-a', () =>
        ok(cancelled ? job('cancelled', 'cancelled') : job('running', 'running'))
      ),
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

    await userEvent.click(await screen.findByRole('button', { name: 'Build stack previews' }));

    const stop = await screen.findByRole('button', { name: 'Stop' });
    await userEvent.click(stop);

    await waitFor(() => expect(cancelled).toBe(true));
    expect(await screen.findByText('Stack stopped')).toBeInTheDocument();
    // The stop button goes once the job is no longer in flight.
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument()
    );
  });

  it('offers no stop button when nothing is building', async () => {
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/latest', () => ok({
        schema_version: 2,
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

    await screen.findByRole('button', { name: 'Build stack previews' });
    expect(screen.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();
  });
});
