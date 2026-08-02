import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import StackPreviewPanel from '../StackPreviewPanel';

const images = [
  { id: 1, target_id: 42, target_name: 'Sh2 86', filter_name: 'Ha', grading_status: 1 },
  { id: 2, target_id: 42, target_name: 'Sh2 86', filter_name: 'Ha', grading_status: 1 },
  { id: 3, target_id: 42, target_name: 'Sh2 86', filter_name: 'OIII', grading_status: 1 },
  { id: 4, target_id: 42, target_name: 'Sh2 86', filter_name: 'OIII', grading_status: 1 },
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

function group(index: number, filter: string, state: string, imageIds: number[]) {
  return {
    index,
    target_id: 42,
    target_name: 'Sh2 86',
    filter_name: filter,
    state,
    phase: state === 'running' ? 'stacking' : state,
    total_candidates: imageIds.length,
    eligible_frames: imageIds.length,
    quality_excluded: 0,
    missing_files: 0,
    processed_frames: state === 'running' ? 1 : 0,
    accepted_frames: state === 'running' ? 1 : 0,
    rejected_frames: 0,
    output_channels: 1,
    reference_image_id: imageIds[0],
    total_exposure_seconds: 0,
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
    input_images: imageIds.map((id) => ({ image_id: id, grading_status: 1 })),
    frames: [],
  };
}

function job(jobId: string, created: number, state: string, groups: unknown[]) {
  return {
    schema_version: 2,
    job_id: jobId,
    database_id: 'test',
    project_id: 1,
    state,
    accepted_only: false,
    created_unix_seconds: created,
    artifact_revision: `rev-${jobId}`,
    cache_version: 7,
    stacking_version: '0.3.0',
    groups,
    error: null,
  };
}

describe('StackPreviewPanel manual queue', () => {
  it('keeps other channels buildable and shows both queued builds', async () => {
    const jobs: Record<string, unknown> = {};
    server.use(
      http.get('/api/stack-activity', () => ok({ schema_version: 1, active: [] })),
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
      http.post(
        '/api/db/:dbId/projects/:projectId/stack-previews',
        async ({ request }) => {
          const body = (await request.json()) as { image_ids: number[] };
          if (body.image_ids.includes(1)) {
            const started = job('job-ha', 100, 'running', [group(0, 'Ha', 'running', [1, 2])]);
            jobs['job-ha'] = started;
            return ok(started);
          }
          const queued = job('job-oiii', 200, 'queued', [group(0, 'OIII', 'queued', [3, 4])]);
          jobs['job-oiii'] = queued;
          return ok(queued);
        }
      ),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/:jobId', ({ params }) =>
        ok(jobs[params.jobId as string])
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

    // Start the Ha channel; while it runs, OIII must stay buildable.
    const buildButtons = await screen.findAllByRole('button', { name: 'Build channel' });
    expect(buildButtons).toHaveLength(2);
    await userEvent.click(buildButtons[0]);
    await screen.findByText('1/2 frames');

    // The running channel's button is held, the other stays open.
    const afterStart = screen.getAllByRole('button', { name: 'Build channel' });
    const enabled = afterStart.filter((button) => !(button as HTMLButtonElement).disabled);
    expect(afterStart).toHaveLength(2);
    expect(enabled).toHaveLength(1);

    // Queue the second channel behind the first.
    await userEvent.click(enabled[0]);
    expect(await screen.findByText('2 builds in the queue')).toBeInTheDocument();
    expect((await screen.findAllByText('Waiting for stacker')).length).toBeGreaterThan(0);
    expect(screen.getByRole('button', { name: 'Stop all (2)' })).toBeEnabled();

    // The whole-set button never greyed out either.
    expect(screen.getByRole('button', { name: 'Build stack previews' })).toBeEnabled();
  });

  it('labels a resumed build with its restored frames', async () => {
    const resumedGroup = {
      ...group(0, 'Ha', 'running', [1, 2]),
      processed_frames: 2,
      accepted_frames: 2,
      reused_frames: 2,
      eligible_frames: 3,
      total_candidates: 3,
    };
    server.use(
      http.get('/api/stack-activity', () => ok({
        schema_version: 1,
        active: [{
          kind: 'mono',
          job_id: 'job-resume',
          database_id: 'test',
          project_id: 1,
          state: 'running',
          label: 'Sh2 86 · Ha',
          detail: 'Registering frames',
          processed_units: 2,
          total_units: 3,
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
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/job-resume', () =>
        ok(job('job-resume', 100, 'running', [resumedGroup]))
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

    expect(await screen.findByText('2/3 frames · 2 resumed')).toBeInTheDocument();
  });
});
