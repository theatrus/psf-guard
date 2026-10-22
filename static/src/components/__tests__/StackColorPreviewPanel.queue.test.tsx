import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackColorJob, StackColorRole, StackColorSource } from '../../api/types';
import StackColorPreviewPanel from '../StackColorPreviewPanel';

const ok = (data: unknown, status = 200) =>
  HttpResponse.json({ success: true, data, error: null }, { status });
const roles: StackColorRole[] = ['red', 'green', 'blue'];
const sources: StackColorSource[] = roles.map((role, index) => ({
  role, filter_name: role, label: role, exposure_group: null,
  job_id: `${role}-stack`, group_index: index, artifact_revision: 'rev', accepted_frames: 4,
  reference_image_id: 1, sky_orientation: null, registration_transform: null,
}));

function job(overrides: Partial<StackColorJob>): StackColorJob {
  return {
    schema_version: 1, job_id: 'color', database_id: 'test', project_id: 1,
    target_id: 42, target_name: 'M31', kind: 'rgb', palette: null, label: 'RGB',
    state: 'completed', phase: 'Ready', sources, source_family_key: 'family',
    created_unix_seconds: 1, artifact_revision: 'color-rev', processed_channels: 3, total_channels: 3,
    progress: { completed_units: 3, total_units: 3, active_phase: null, current_role: null, current_stage: null, stage_count: null, phases: [] },
    cache_version: 12, stacking_version: 'test', background_version: 'test', deconvolution_version: '',
    linear_input_id: null, crop: 'none', crop_report: null, processing: null,
    resolved_input_stretches: {}, resolved_input_deconvolutions: {}, resolved_output_stretches: [],
    resolved_backgrounds: {}, resolved_background_protection: {}, background_protection_fallbacks: {},
    preview_url: '/preview', fits_url: '/fits', outdated: false, outdated_reason: null, error: null,
    ...overrides,
  };
}

/** A composition being prepared, waiting for its channel builds. */
const preparing = job({
  job_id: 'slot', state: 'preparing', phase: 'Waiting for the channel builds to finish',
  preparing: 'RGB', sources: [], target_name: '', processed_channels: 0,
  progress: { completed_units: 0, total_units: 0, active_phase: null, current_role: null, current_stage: null, stage_count: null, phases: [] },
});

function mount(slot: () => Response) {
  server.use(
    http.get('/api/stack-activity', () => ok({ active: [] })),
    http.get('/api/db/test/projects/1/stack-previews/color', () => ok({
      targets: [{
        target_id: 42, target_name: 'M31', available_roles: roles.map((role) => ({ role, filter_name: role })),
        source_candidates: sources, ambiguous_roles: [], unmapped_filters: [],
        rgb_available: true, lrgb_available: false, narrowband_palettes: [],
      }],
      jobs: [],
    })),
    http.post('/api/db/test/projects/1/stack-previews/color', () => ok(preparing, 202)),
    http.get('/api/db/test/projects/1/stack-previews/color/slot', slot),
    http.get('/api/db/test/projects/1/stack-previews/color/real', () => ok(job({ job_id: 'real' }))),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}>
    <StackColorPreviewPanel dbId="test" projectId={1} sourceRevision="initial"
      channelBuildRunning={false} outdatedSourceKeys={new Set()} canCompute onOpenImage={() => undefined} />
  </QueryClientProvider>);
}

describe('a color composition being prepared', () => {
  it('shows on the card that asked, then follows the composition it became', async () => {
    let stage: 'preparing' | 'replaced' = 'preparing';
    mount(() => ok(stage === 'preparing'
      ? preparing
      : { ...preparing, state: 'replaced', preparing: undefined, replaced_by: ['real'] }));
    await userEvent.click(await screen.findByRole('button', { name: 'Build RGB color preview' }));
    const build = await screen.findByRole('button', { name: 'Build RGB color preview' });
    await waitFor(() => expect(build).toHaveTextContent('Preparing…'));
    expect(build).toBeDisabled();
    expect(build.closest('article')).toHaveTextContent('Waiting for the channel builds to finish');

    stage = 'replaced';
    const rebuild = await screen.findByRole('button', { name: 'Rebuild RGB color preview' }, { timeout: 3000 });
    expect(rebuild).toBeEnabled();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('lets go of one the server no longer keeps, without an error', async () => {
    mount(() => HttpResponse.json({ success: false, data: null, error: 'Not found' }, { status: 404 }));
    await userEvent.click(await screen.findByRole('button', { name: 'Build RGB color preview' }));
    const build = screen.getByRole('button', { name: 'Build RGB color preview' });
    await waitFor(() => expect(build).toHaveTextContent('Build'), { timeout: 3000 });
    await waitFor(() => expect(build).toBeEnabled());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('shows why one could not be prepared', async () => {
    let stage: 'preparing' | 'failed' = 'preparing';
    mount(() => ok(stage === 'preparing' ? preparing : {
      ...preparing, state: 'failed', phase: 'Could not be prepared', preparing: undefined,
      error: 'RC-Astro on the color channels cannot run: rc-astro is not installed on this server',
    }));
    await userEvent.click(await screen.findByRole('button', { name: 'Build RGB color preview' }));
    const build = await screen.findByRole('button', { name: 'Build RGB color preview' });
    await waitFor(() => expect(build).toHaveTextContent('Preparing…'));
    stage = 'failed';
    // The card has no preview yet, so the reason shows once, in its place.
    const message = 'RC-Astro on the color channels cannot run: rc-astro is not installed on this server';
    expect(await screen.findByText(message, {}, { timeout: 3000 }))
      .toHaveClass('stack-preview-placeholder', 'error');
    expect(screen.getAllByText(message)).toHaveLength(1);
    await waitFor(() => expect(screen.getByRole('button', { name: /RGB color preview/ })).toBeEnabled());
  });
});
