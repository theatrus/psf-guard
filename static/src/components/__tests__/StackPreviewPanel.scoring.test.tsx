import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { SCORING_DEFAULTS } from '../../hooks/useScoringPreferences';
import {
  deliverScoringDefaults,
  scoringServer,
  testQueryClient,
  waitForScoringDefaults,
} from '../../test/scoringDefaults';
import type { StackScoringSettings } from '../../api/types';
import StackPreviewPanel from '../StackPreviewPanel';

const images = [
  { id: 1, target_id: 42, target_name: 'Sh2 86', filter_name: 'Ha', grading_status: 1 },
  { id: 2, target_id: 42, target_name: 'Sh2 86', filter_name: 'Ha', grading_status: 1 },
];

function wrapper(queryClient: QueryClient, entry = '/stacks?db=test&project=1') {
  return function Wrapper({ children }: { children: ReactNode }) {
    // The panel scores with the link's values over the server's defaults.
    return (
      <MemoryRouter initialEntries={[entry]}>
        <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
      </MemoryRouter>
    );
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

function renderPanel(entry?: string) {
  const client = testQueryClient();
  const view = render(
    <StackPreviewPanel
      dbId="test"
      projectId={1}
      images={images}
      selectionSource="visible"
      onOpenImage={() => undefined}
    />,
    { wrapper: wrapper(client, entry) }
  );
  return { client, ...view };
}

describe('StackPreviewPanel scoring provenance', () => {
  it('treats an old artifact without scoring metadata as calibrated', async () => {
    mockLatest();
    const view = renderPanel();

    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-card'))
        .toHaveAttribute('data-outdated', 'false')
    );
  });

  it('marks a remembered artifact stale when the scoring defaults change', async () => {
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
      deliverScoringDefaults(view.client, { ...SCORING_DEFAULTS, satellite: 0 });
    });

    await waitFor(() =>
      expect(view.container.querySelector('.stack-preview-outdated'))
        .toHaveTextContent('Out of date — scoring settings changed')
    );
  });
});

describe('StackPreviewPanel and a view\'s own scoring', () => {
  const VIEW = '/stacks?db=test&project=1&penaltySatellite=0&penaltyPointing=1'
    + '&penaltyTemporal=1&maxHfr=off&minStars=40';

  function mockBuild() {
    const submitted: Record<string, unknown>[] = [];
    server.use(
      http.get('/api/stack-activity', () => ok({ schema_version: 1, active: [] })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/latest', () => ok({ groups: [] })),
      http.get('/api/db/:dbId/projects/:projectId/stack-previews/color', () => ok({ targets: [], jobs: [] })),
      http.post('/api/db/:dbId/projects/:projectId/stack-previews/channels', async ({ request }) => {
        submitted.push(await request.json() as Record<string, unknown>);
        return ok([]);
      }),
    );
    return submitted;
  }

  it('builds with the link\'s scoring, and says so beside Build', async () => {
    scoringServer.set({ ...SCORING_DEFAULTS, hfrRejectAbove: 3 });
    const submitted = mockBuild();
    renderPanel(VIEW);
    const note = await screen.findByRole('note');
    // The server's defaults arrive after the first render.
    await waitFor(() => expect(note).toHaveTextContent('Max HFR off (default 3)'));
    expect(note).toHaveTextContent('Builds here use this view\'s scoring, not the defaults');
    expect(note).toHaveTextContent('Satellite trails off (default 100%)');
    expect(note).toHaveTextContent('Max HFR off (default 3)');
    expect(note).toHaveTextContent('Min stars 40 (default off)');
    await userEvent.click(screen.getByRole('button', { name: 'Build stacks' }));
    await waitFor(() => expect(submitted).toHaveLength(1));
    expect(submitted[0].scoring).toEqual({ penalty_satellite: 0, star_count_reject_below: 40 });
  });

  it('builds with the defaults once the view is reset', async () => {
    scoringServer.set({ ...SCORING_DEFAULTS, hfrRejectAbove: 3 });
    const submitted = mockBuild();
    const { client } = renderPanel(VIEW);
    await waitForScoringDefaults(client);
    await userEvent.click(await screen.findByRole('button', { name: 'Reset to default' }));
    await waitFor(() => expect(screen.queryByRole('note')).toBeNull());
    await userEvent.click(screen.getByRole('button', { name: 'Build stacks' }));
    await waitFor(() => expect(submitted).toHaveLength(1));
    expect(submitted[0].scoring).toEqual({ hfr_reject_above: 3 });
  });

  it('says nothing while the view scores as the defaults do', async () => {
    mockBuild();
    renderPanel();
    await screen.findByRole('button', { name: 'Build stacks' });
    expect(screen.queryByRole('note')).toBeNull();
  });
});
