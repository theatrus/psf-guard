import { describe, expect, it } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackColorRole, StackColorSource } from '../../api/types';
import StackColorPreviewPanel from '../StackColorPreviewPanel';
import { colorSourceKey } from '../stackColorSources';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const roles: StackColorRole[] = ['red', 'green', 'blue'];
const candidates: StackColorSource[] = roles.flatMap((role, index) => [30, 300].map((seconds) => ({
  role, filter_name: role, label: `${role} (${seconds} s)`,
  exposure_group: { key: `${seconds}`, label: `${seconds} s`, min_seconds: seconds, max_seconds: seconds },
  job_id: `${role}-${seconds}`, group_index: index, artifact_revision: 'rev', accepted_frames: 4,
  reference_image_id: 1, sky_orientation: null, registration_transform: null,
})));

const target = {
  target_id: 42, target_name: 'M31', available_roles: [], source_candidates: candidates,
  ambiguous_roles: roles, unmapped_filters: [], rgb_available: true, lrgb_available: false, narrowband_palettes: [],
};

describe('color exposure source selection', () => {
  it('keeps stale explicit selectors visible after exposure grouping is disabled', async () => {
    let currentCandidates = candidates;
    server.use(
      http.get('/api/stack-activity', () => ok({ active: [] })),
      http.get('/api/db/test/projects/1/stack-previews/color', () => ok({
        targets: [{ ...target, source_candidates: currentCandidates,
          ambiguous_roles: currentCandidates.length > roles.length ? roles : [] }], jobs: [],
      })),
    );
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const panel = (revision: string) => <QueryClientProvider client={queryClient}>
      <StackColorPreviewPanel dbId="test" projectId={1} sourceRevision={revision}
        channelBuildRunning={false} outdatedTargetIds={new Set()} canCompute onOpenImage={() => undefined} />
    </QueryClientProvider>;
    const rendered = render(panel('split'));
    const selector = (role: StackColorRole) => screen.getByRole('combobox', {
      name: `M31 rgb ${role[0].toUpperCase()} source stack`,
    });
    await screen.findByRole('button', { name: 'Build RGB color preview' });
    for (const role of roles) {
      const source = candidates.find((candidate) => candidate.role === role && candidate.exposure_group?.key === '300')!;
      await userEvent.selectOptions(selector(role), colorSourceKey(source));
    }
    expect(screen.getByRole('button', { name: 'Build RGB color preview' })).toBeEnabled();
    currentCandidates = roles.map((role) => ({
      ...candidates.find((candidate) => candidate.role === role)!,
      job_id: `${role}-unsplit`, exposure_group: null, label: role,
    }));
    rendered.rerender(panel('unsplit'));
    await waitFor(() => expect(selector('red')).toHaveValue(''));
    expect(screen.getByRole('button', { name: 'Build RGB color preview' })).toBeDisabled();
    for (const source of currentCandidates) {
      expect(selector(source.role)).toBeVisible();
      await userEvent.selectOptions(selector(source.role), colorSourceKey(source));
    }
    expect(screen.getByRole('button', { name: 'Build RGB color preview' })).toBeEnabled();
  });

  it('identifies the saved exposure sources in the full-size inspector', async () => {
    const sources = candidates.filter((source) => source.exposure_group?.key === '300');
    const job = {
      schema_version: 1, job_id: 'color-long', database_id: 'test', project_id: 1,
      target_id: 42, target_name: 'M31', kind: 'rgb', palette: null, label: 'RGB',
      state: 'completed', phase: 'Ready', sources,
      created_unix_seconds: 1, artifact_revision: 'color-rev', processed_channels: 3, total_channels: 3,
      preview_url: '/preview', fits_url: '/fits', outdated: false,
    };
    server.use(
      http.get('/api/stack-activity', () => ok({ active: [] })),
      http.get('/api/db/test/projects/1/stack-previews/color', () => ok({
        targets: [{ ...target, source_candidates: sources, ambiguous_roles: [] }], jobs: [job],
      })),
    );
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<QueryClientProvider client={queryClient}>
      <StackColorPreviewPanel dbId="test" projectId={1} sourceRevision="test"
        channelBuildRunning={false} outdatedTargetIds={new Set()} canCompute onOpenImage={() => undefined} />
    </QueryClientProvider>);
    await userEvent.click(await screen.findByRole('button', { name: 'Inspect RGB full size' }));
    const inspector = screen.getByRole('dialog');
    expect(within(inspector).getByText('red (300 s), green (300 s), blue (300 s)')).toBeVisible();
  });

  it('requires explicit ambiguous choices and submits the selected artifact identities', async () => {
    let submitted: Record<string, unknown> | undefined;
    const job = {
      schema_version: 1, job_id: 'color', database_id: 'test', project_id: 1,
      target_id: 42, target_name: 'M31', kind: 'rgb', palette: null, label: 'RGB',
      state: 'queued', phase: 'Waiting', sources: candidates.filter((source) => source.exposure_group?.key === '300'),
      created_unix_seconds: 1, artifact_revision: 'color-rev', processed_channels: 0, total_channels: 3,
      preview_url: '/preview', fits_url: '/fits', outdated: false,
    };
    server.use(
      http.get('/api/stack-activity', () => ok({ active: [] })),
      http.get('/api/db/test/projects/1/stack-previews/color', () => ok({ targets: [target], jobs: [] })),
      http.post('/api/db/test/projects/1/stack-previews/color', async ({ request }) => {
        submitted = await request.json() as Record<string, unknown>;
        return ok(job);
      }),
      http.get('/api/db/test/projects/1/stack-previews/color/color', () => ok(job)),
    );
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<QueryClientProvider client={queryClient}>
      <StackColorPreviewPanel dbId="test" projectId={1} sourceRevision="test"
        channelBuildRunning={false} outdatedTargetIds={new Set()} canCompute onOpenImage={() => undefined} />
    </QueryClientProvider>);
    const build = await screen.findByRole('button', { name: 'Build RGB color preview' });
    expect(build).toBeDisabled();
    for (const role of roles) {
      const source = candidates.find((candidate) => candidate.role === role && candidate.exposure_group?.key === '300')!;
      await userEvent.selectOptions(screen.getByRole('combobox', {
        name: `M31 rgb ${role[0].toUpperCase()} source stack`,
      }), colorSourceKey(source));
    }
    expect(build).toBeEnabled();
    await userEvent.click(build);
    await waitFor(() => expect(submitted?.input_sources).toEqual(Object.fromEntries(roles.map((role, index) => [role, {
      job_id: `${role}-300`, group_index: index, artifact_revision: 'rev',
    }]))));
    expect(screen.getByRole('status')).toHaveTextContent('Waiting');
  });
});
