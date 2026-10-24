import { describe, expect, it } from 'vitest';
import { render, screen, waitFor, within, act } from '@testing-library/react';
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


describe('automatic narrowband on a regular card', () => {
  async function setup(legacy: boolean | 'rgb-stars' = false) {
    const nbRoles: StackColorRole[] = ['ha', 'oiii', 'sii'];
    const candidates: StackColorSource[] = [...sources, ...nbRoles.map((role, index) => ({
      ...sources[0], role, filter_name: role, job_id: `${role}-stack`, group_index: index,
    })), { ...sources[0], job_id: 'red-short', group_index: 9 }];
    const previous: StackColorJob = { ...queued, kind: 'narrowband', palette: 'sho',
      job_id: 'previous', state: 'completed', sources: candidates.filter((s) => nbRoles.includes(s.role)),
      processing: { ...defaultColorProcessing(nbRoles), ...(legacy === 'rgb-stars' ? { input_rc_astro: { red: { steps: [{ tool: 'bxt', parameters: {} }] } } } : {}) },
      ...(legacy === 'rgb-stars' ? { stars: 'rgb' as const, automatic_palette: true } : {}),
      preview_url: '/preview.png', artifact_revision: 'old',
    };
    const started: Record<string, unknown>[] = [];
    server.use(
      http.get('/api/stack-activity', () => ok({ active: [] })),
      http.get('/api/processing-setups', () => ok({ schema_version: 1, setups: [] })),
      http.get('/api/db/test/projects/:projectId/stack-previews/color', () => ok({
        targets: [{ target_id: 42, target_name: 'M31',
          available_roles: [...roles, ...nbRoles].map((role) => ({ role, filter_name: role })),
          source_candidates: candidates, ambiguous_roles: ['red'], unmapped_filters: [],
          rgb_available: true, lrgb_available: false, narrowband_palettes: ['sho', 'hoo', 'foraxx-sho'],
        }], jobs: legacy ? [previous] : [],
      })),
      http.post('/api/db/test/projects/1/stack-previews/color', async ({ request }) => {
        const body = await request.json() as Record<string, unknown>;
        started.push(body);
        return ok({ ...previous, ...body, job_id: 'new', state: 'queued' }, 202);
      }),
      http.get('/api/db/test/projects/1/stack-previews/color/new', () => ok({ ...previous, state: 'queued', job_id: 'new' })),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const view = render(<QueryClientProvider client={client}>
      <StackColorPreviewPanel dbId="test" projectId={1} sourceRevision="initial" channelBuildRunning={false}
        outdatedSourceKeys={new Set()} canCompute onOpenImage={() => undefined} />
    </QueryClientProvider>);
    await screen.findByRole('combobox', { name: 'M31 narrowband palette' });
    const card = view.container.querySelector<HTMLElement>('article[data-color-kind="narrowband"]')!;
    expect(card.closest('.stack-color-grid')).not.toBeNull();
    expect(card.closest('details[data-color-kind]')).toBeNull();
    return { card, started,
      changeProject: () => view.rerender(<QueryClientProvider client={client}>
        <StackColorPreviewPanel dbId="test" projectId={2} sourceRevision="initial" channelBuildRunning={false}
          outdatedSourceKeys={new Set()} canCompute onOpenImage={() => undefined} />
      </QueryClientProvider>),
      removeRgbSource: async () => {
        candidates.splice(candidates.findIndex((source) => source.job_id === 'red-short'), 1);
        await act(() => client.refetchQueries());
      },
    };
  }

  it('defaults to automatic palette and RGB stars, with exact RGB choices inside Custom combination', async () => {
    const { card, started } = await setup();
    const ui = within(card);
    expect(ui.getByRole('combobox', { name: /narrowband palette/ })).toHaveValue('automatic');
    expect(ui.getByRole('combobox', { name: /SHO stars/ })).toHaveValue('automatic');
    const custom = ui.getByText('Custom combination').closest('details')!;
    expect(custom).not.toHaveAttribute('open');
    await userEvent.click(ui.getByText('Custom combination'));
    await userEvent.selectOptions(ui.getByRole('combobox', { name: /R RGB stars source stack/ }),
      Array.from(ui.getByRole<HTMLSelectElement>('combobox', { name: /R RGB stars source stack/ }).options)
        .find((option) => option.value.includes('red-short'))!.value);
    await userEvent.selectOptions(ui.getByRole('combobox', { name: /edge crop/ }), 'inscribed');
    await userEvent.selectOptions(ui.getByRole('combobox', { name: /SHO stars/ }), 'rgb');
    expect(ui.getByRole('combobox', { name: /narrowband palette/ })).toHaveValue('automatic');
    expect(ui.getByRole('combobox', { name: /edge crop/ })).toHaveValue('inscribed');
    await userEvent.click(ui.getByRole('button', { name: 'Build SHO color preview' }));
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0]).toMatchObject({ kind: 'narrowband', palette: 'sho', automatic_palette: true,
      stars: 'rgb', crop: 'inscribed', input_sources: { red: { job_id: 'red-short', group_index: 9, artifact_revision: 'rev' } },
      processing: { narrowband: { shared_stretch: true, balance: { oiii_boost: 1, sii_boost: 1, shadow_protection: 1 } } },
    });
  });

  it('resets Stars when another project has the same target identifier', async () => {
    const { card, changeProject } = await setup();
    await userEvent.selectOptions(within(card).getByRole('combobox', { name: /SHO stars/ }), 'none');
    changeProject();
    await waitFor(() => expect(screen.getByRole('combobox', { name: /SHO stars/ })).toHaveValue('automatic'));
  });

  it('keeps the selected RGB source when changing palettes', async () => {
    const { card, started } = await setup();
    const ui = within(card);
    await userEvent.click(ui.getByText('Custom combination'));
    const picker = ui.getByRole<HTMLSelectElement>('combobox', { name: /R RGB stars source stack/ });
    const chosen = Array.from(picker.options).find((option) => option.value.includes('red-short'))!.value;
    await userEvent.selectOptions(picker, chosen);
    await userEvent.selectOptions(ui.getByRole('combobox', { name: /narrowband palette/ }), 'hoo');
    const nextCard = screen.getByRole('combobox', { name: /narrowband palette/ }).closest('article')!;
    await userEvent.click(within(nextCard).getByText('Custom combination'));
    expect(within(nextCard).getByRole('combobox', { name: /R RGB stars source stack/ })).toHaveValue(chosen);
    await userEvent.click(within(nextCard).getByRole('button', { name: 'Build HOO color preview' }));
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0]).toMatchObject({ input_sources: { red: { job_id: 'red-short' } } });
  });

  it('allows narrowband stars after a selected RGB source disappears', async () => {
    const { card, removeRgbSource } = await setup();
    const ui = within(card);
    await userEvent.click(ui.getByText('Custom combination'));
    const picker = ui.getByRole<HTMLSelectElement>('combobox', { name: /R RGB stars source stack/ });
    await userEvent.selectOptions(picker, Array.from(picker.options).find((option) => option.value.includes('red-short'))!.value);
    await removeRgbSource();
    expect(ui.getByRole('button', { name: 'Build SHO color preview' })).toBeDisabled();
    await userEvent.selectOptions(ui.getByRole('combobox', { name: /SHO stars/ }), 'narrowband');
    expect(ui.getByRole('button', { name: 'Build SHO color preview' })).toBeEnabled();
  });

  it('drops unused RGB tool settings when changing a saved RGB-stars preview to narrowband stars', async () => {
    const { card, started } = await setup('rgb-stars');
    const ui = within(card);
    await userEvent.selectOptions(ui.getByRole('combobox', { name: /SHO stars/ }), 'narrowband');
    await userEvent.click(ui.getByRole('button', { name: /Rebuild SHO color preview/ }));
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0]).toMatchObject({ stars: 'narrowband', processing: { input_rc_astro: {} } });
    expect(started[0].input_sources ?? {}).not.toHaveProperty('red');
  });

  it('keeps old saved palettes, channel-card stars, and processing when rebuilt', async () => {
    const { card, started } = await setup(true);
    const ui = within(card);
    expect(ui.getByRole('combobox', { name: /narrowband palette/ })).toHaveValue('sho');
    expect(ui.getByRole('combobox', { name: /SHO stars/ })).toHaveValue('channels');
    await userEvent.click(ui.getByRole('button', { name: /Rebuild SHO color preview/ }));
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0]).not.toHaveProperty('automatic_palette');
    expect(started[0]).not.toHaveProperty('stars');
    expect(started[0].processing).not.toHaveProperty('narrowband');
  });
});
