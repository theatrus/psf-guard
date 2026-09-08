import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
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
  it('restores revision-scoped processing on reload and durably reverts without running tools', async () => {
    const user = userEvent.setup();
    let posts = 0;
    let selected: unknown = {
      schema_version: 3, stretch_id: 'selected', stretch_version: '0.1',
      config: { model: { type: 'identity' }, color_strategy: 'linked', max_analysis_samples: 4 },
      request: { model: { type: 'identity' }, color_strategy: 'linked',
        rc_astro: { steps: [{ tool: 'sxt', parameters: { stars: true } }] } },
      deconvolution: null, input_range: null,
      linked_statistics: { min: 0, max: 1, median: 0.2 },
      rc_astro: { cli_version: '2.6.6', has_stars: true, steps: [{
        tool: 'sxt', name: 'StarXTerminator', ml_version: 11, warnings: [],
      }] },
      preview_url: '/selected.png', original_preview_url: '/selected-full.png',
      stars_preview_url: '/stars.png', fits_url: '/selected.fits',
    };
    const revisions: string[] = [];
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/latest', () => ok({
        groups: [{ job_id: 'ready-job', artifact_revision: 'ready-revision', accepted_only: false,
          group: { ...runningGroup, state: 'ready', phase: 'ready' } }],
      })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({ targets: [], jobs: [] })),
      http.get('/api/db/test/stack-previews/ready-job/0/stretch', ({ request }) => {
        revisions.push(new URL(request.url).searchParams.get('v') ?? '');
        return ok(selected);
      }),
      http.delete('/api/db/test/stack-previews/ready-job/0/stretch', ({ request }) => {
        expect(new URL(request.url).searchParams.get('v')).toBe('ready-revision');
        selected = null;
        return new HttpResponse(null, { status: 204 });
      }),
      http.post('/api/db/test/stack-previews/ready-job/0/stretch', () => { posts += 1; return ok(selected); })
    );
    const panel = <StackPreviewPanel dbId="test" projectId={1} images={images}
      selectionSource="visible" onOpenImage={() => undefined} />;
    const first = render(panel, { wrapper: wrapper() });
    await waitFor(() => expect(screen.getByAltText('Sh2 86 Ha stack preview')).toHaveAttribute('src', '/selected.png'));
    first.unmount();
    const second = render(panel, { wrapper: wrapper() });
    await waitFor(() => expect(screen.getByAltText('Sh2 86 Ha stack preview')).toHaveAttribute('src', '/selected.png'));
    await user.click(screen.getByText('View processing'));
    expect(screen.getByText('RC-Astro 2.6.6')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Revert processing' }));
    await waitFor(() => expect(screen.getByAltText('Sh2 86 Ha stack preview')).not.toHaveAttribute('src', '/selected.png'));
    second.unmount();
    render(panel, { wrapper: wrapper() });
    await waitFor(() => expect(revisions).toEqual(['ready-revision', 'ready-revision', 'ready-revision']));
    expect(screen.getByAltText('Sh2 86 Ha stack preview')).not.toHaveAttribute('src', '/selected.png');
    expect(posts).toBe(0);
  });
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
    // The queue stays open while the adopted build runs: the header button
    // keeps its label and stays clickable, and a Stop appears for the build.
    expect(screen.getByRole('button', { name: 'Build stack previews' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled();
  });

  it('drops a finished build for one that is still running', async () => {
    let activity: unknown[] = [];
    const readyGroup = {
      ...runningGroup,
      state: 'ready',
      phase: 'ready',
      processed_frames: 2,
      accepted_frames: 2,
    };
    const otherGroup = {
      ...runningGroup,
      filter_name: 'OIII',
      eligible_frames: 3,
      total_candidates: 3,
    };
    server.use(
      http.get('/api/stack-activity', () => ok({ schema_version: 1, active: activity })),
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
      http.post('/api/db/:dbId/projects/:projectId/stack-previews', () => ok({
        schema_version: 2,
        job_id: 'job-done',
        database_id: 'test',
        project_id: 1,
        state: 'completed',
        accepted_only: false,
        created_unix_seconds: 100,
        artifact_revision: 'rev-done',
        cache_version: 7,
        stacking_version: '0.2.0',
        groups: [readyGroup],
        error: null,
      })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/job-done', () => ok({
        schema_version: 2,
        job_id: 'job-done',
        database_id: 'test',
        project_id: 1,
        state: 'completed',
        accepted_only: false,
        created_unix_seconds: 100,
        artifact_revision: 'rev-done',
        cache_version: 7,
        stacking_version: '0.2.0',
        groups: [readyGroup],
        error: null,
      })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/job-other', () => ok({
        schema_version: 2,
        job_id: 'job-other',
        database_id: 'test',
        project_id: 1,
        state: 'running',
        accepted_only: false,
        created_unix_seconds: 200,
        artifact_revision: 'rev-other',
        cache_version: 7,
        stacking_version: '0.2.0',
        groups: [otherGroup],
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

    await userEvent.click(await screen.findByRole('button', { name: 'Build stack previews' }));
    expect(await screen.findByText('2/2 frames')).toBeInTheDocument();

    // Another build starts — from a second tab, or the desktop app. The
    // finished job this panel is showing must not hide it.
    activity = [{
      kind: 'mono',
      job_id: 'job-other',
      database_id: 'test',
      project_id: 1,
      state: 'running',
      label: 'Sh2 86 · OIII',
      detail: 'Registering frames',
      processed_units: 1,
      total_units: 3,
      created_unix_seconds: 200,
    }];

    expect(await screen.findByText('1/3 frames', {}, { timeout: 8_000 })).toBeInTheDocument();
  }, 15_000);
});
