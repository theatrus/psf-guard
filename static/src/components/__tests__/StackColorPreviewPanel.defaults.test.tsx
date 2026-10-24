import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackColorJob, StackColorProcessing, StackColorRole, StackColorSource } from '../../api/types';
import StackColorPreviewPanel from '../StackColorPreviewPanel';
import { ALL_COLOR_ROLES, defaultColorProcessing } from '../stackColorProcessing';

const ok = (data: unknown, status = 200) =>
  HttpResponse.json({ success: true, data, error: null }, { status });
const roles: StackColorRole[] = ['red', 'green', 'blue'];
const sources: StackColorSource[] = roles.map((role, index) => ({
  role, filter_name: role, label: role, exposure_group: null,
  job_id: `${role}-stack`, group_index: index, artifact_revision: 'rev', accepted_frames: 4,
  reference_image_id: 1, sky_orientation: null, registration_transform: null,
}));

const queued: StackColorJob = {
  schema_version: 1, job_id: 'color', database_id: 'test', project_id: 1,
  target_id: 42, target_name: 'M31', kind: 'rgb', palette: null, label: 'RGB',
  state: 'queued', phase: 'Queued', sources, source_family_key: 'family',
  created_unix_seconds: 1, artifact_revision: '', processed_channels: 0, total_channels: 3,
  progress: { completed_units: 0, total_units: 3, active_phase: null, current_role: null, current_stage: null, stage_count: null, phases: [] },
  cache_version: 12, stacking_version: 'test', background_version: 'test', deconvolution_version: '',
  linear_input_id: null, crop: 'none', crop_report: null, processing: null,
  resolved_input_stretches: {}, resolved_input_deconvolutions: {}, resolved_output_stretches: [],
  resolved_backgrounds: {}, resolved_background_protection: {}, background_protection_fallbacks: {},
  preview_url: '', fits_url: '', outdated: false, outdated_reason: null, error: null,
};

describe('a color card with no preview yet', () => {
  it('builds from the default Color setup, and shows the build pending while it reads it', async () => {
    let release = () => {};
    const released = new Promise<void>((resolve) => { release = resolve; });
    const faint: StackColorProcessing = {
      ...defaultColorProcessing(ALL_COLOR_ROLES),
      background_extraction: null,
    };
    const started: { processing: StackColorProcessing }[] = [];
    server.use(
      http.get('/api/stack-activity', () => ok({ active: [] })),
      http.get('/api/processing-setups', async () => {
        await released;
        return ok({ schema_version: 1, setups: [], default_color: 'Faint', default_color_processing: faint });
      }),
      http.get('/api/db/test/projects/1/stack-previews/color', () => ok({
        targets: [{
          target_id: 42, target_name: 'M31', available_roles: roles.map((role) => ({ role, filter_name: role })),
          source_candidates: sources, ambiguous_roles: [], unmapped_filters: [],
          rgb_available: true, lrgb_available: false, narrowband_palettes: [],
        }],
        jobs: [],
      })),
      http.post('/api/db/test/projects/1/stack-previews/color', async ({ request }) => {
        started.push(await request.json() as { processing: StackColorProcessing });
        return ok(queued, 202);
      }),
      http.get('/api/db/test/projects/1/stack-previews/color/color', () => ok(queued)),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<QueryClientProvider client={client}>
      <StackColorPreviewPanel dbId="test" projectId={1} sourceRevision="initial"
        channelBuildRunning={false} outdatedSourceKeys={new Set()} canCompute onOpenImage={() => undefined} />
    </QueryClientProvider>);

    const build = await screen.findByRole('button', { name: 'Build RGB color preview' });
    await userEvent.click(build);
    // Pending at once, so a second click cannot start it twice.
    expect(build).toHaveTextContent('Building…');
    expect(build).toBeDisabled();
    await userEvent.click(build);
    expect(started).toHaveLength(0);

    release();
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0].processing.background_extraction).toBeNull();
    expect(Object.keys(started[0].processing.input_stretches)).toEqual(['red', 'green', 'blue']);
    expect(started).toHaveLength(1);
  });
});
