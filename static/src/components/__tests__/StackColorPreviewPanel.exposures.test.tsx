import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackColorInputSources, StackColorJob, StackColorProcessing, StackColorRole, StackColorSource } from '../../api/types';
import StackColorPreviewPanel from '../StackColorPreviewPanel';
import { colorSourceKey } from '../stackColorSources';

vi.mock('../StackColorProcessingControls', () => ({
  default: ({ label, applied, disabled, onApply }: {
    label: string; applied: StackColorProcessing | null; disabled: boolean;
    onApply: (processing: StackColorProcessing) => void;
  }) => <button type="button" disabled={disabled} data-applied={JSON.stringify(applied)}
    onClick={() => onApply({ background_extraction: null, input_deconvolutions: {}, input_stretches: {},
      output_stretches: [{ model: { type: 'linear', black: 0, white: 0.7 }, color_strategy: 'linked' }] })}>
    Apply processing {label}
  </button>,
}));

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const roles: StackColorRole[] = ['red', 'green', 'blue'];
const candidates: StackColorSource[] = roles.flatMap((role, index) => [30, 300].map((seconds) => ({
  role, filter_name: role, label: `${role} (${seconds} s)`,
  exposure_group: { key: `${role}-${seconds}`, label: `${seconds} s`, min_seconds: seconds, max_seconds: seconds },
  job_id: `${role}-${seconds}`, group_index: index, artifact_revision: 'rev', accepted_frames: 4,
  reference_image_id: 1, sky_orientation: null, registration_transform: null,
})));
const atSeconds = (seconds: number) => candidates.filter((source) => source.exposure_group?.min_seconds === seconds);
const references = (sources: StackColorSource[]) => Object.fromEntries(sources.map((source) => [source.role, {
  job_id: source.job_id, group_index: source.group_index, artifact_revision: source.artifact_revision,
}]));

function job(sources: StackColorSource[], overrides: Partial<StackColorJob> = {}): StackColorJob {
  return {
    schema_version: 1, job_id: 'color', database_id: 'test', project_id: 1,
    target_id: 42, target_name: 'M31', kind: 'rgb', palette: null, label: 'RGB',
    state: 'completed', phase: 'Ready', sources, source_family_key: sources.map((source) => source.job_id).join(':'),
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

interface BuildRequest {
  input_sources?: StackColorInputSources;
  crop: string;
  processing: StackColorProcessing;
  force: boolean;
}

function mount(options: {
  sources?: StackColorSource[]; jobs?: StackColorJob[]; legacy?: boolean;
  omitTarget?: boolean;
  outdated?: ReadonlySet<string>; response?: (sources: StackColorSource[]) => Partial<StackColorJob>;
} = {}) {
  const catalog = { sources: options.sources ?? candidates, jobs: options.jobs ?? [], legacy: options.legacy ?? false };
  const submissions: BuildRequest[] = [];
  const posted = new Map<string, StackColorJob>();
  server.use(
    http.get('/api/stack-activity', () => ok({ active: [] })),
    http.get('/api/db/test/projects/1/stack-previews/color', () => ok({ targets: options.omitTarget ? [] : [{
      target_id: 42, target_name: 'M31', available_roles: roles.map((role) => ({ role, filter_name: role })),
      ...(catalog.legacy ? {} : { source_candidates: catalog.sources }),
      ambiguous_roles: roles.filter((role) => catalog.sources.filter((source) => source.role === role).length > 1),
      unmapped_filters: [], rgb_available: roles.every((role) => catalog.sources.some((source) => source.role === role)),
      lrgb_available: false, narrowband_palettes: [],
    }], jobs: catalog.jobs })),
    http.post('/api/db/test/projects/1/stack-previews/color', async ({ request }) => {
      const body = await request.json() as BuildRequest;
      submissions.push(body);
      const selected = body.input_sources ? catalog.sources.filter((source) => {
        const reference = body.input_sources?.[source.role];
        return reference?.job_id === source.job_id && reference.group_index === source.group_index
          && reference.artifact_revision === source.artifact_revision;
      }) : catalog.sources;
      const response = job(selected, { job_id: `posted-${submissions.length}`, state: 'queued', phase: 'Waiting',
        created_unix_seconds: submissions.length + 10, processed_channels: 0, ...options.response?.(selected) });
      posted.set(response.job_id, response);
      return ok(response);
    }),
    http.get('/api/db/test/projects/1/stack-previews/color/:jobId', ({ params }) => ok(posted.get(String(params.jobId)))),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const panel = (revision: string, outdated = options.outdated ?? new Set<string>()) => <QueryClientProvider client={client}>
    <StackColorPreviewPanel dbId="test" projectId={1} sourceRevision={revision}
      channelBuildRunning={false} outdatedSourceKeys={outdated} canCompute onOpenImage={() => undefined} />
  </QueryClientProvider>;
  const rendered = render(panel('initial'));
  return { catalog, submissions, rerender: (revision: string, outdated?: ReadonlySet<string>) => rendered.rerender(panel(revision, outdated)) };
}

function cardFor(button: HTMLElement) {
  const article = button.closest('article');
  if (!article) throw new Error('Color action is not inside its card');
  return article;
}

const selector = (role: StackColorRole) => screen.getByRole('combobox', {
  name: `M31 rgb ${role[0].toUpperCase()} source stack`,
});

describe('automatic color exposure cards', () => {
  it('offers independent short and long builds without manual source choices', async () => {
    mount();
    const short = await screen.findByRole('button', { name: 'Build RGB 30 s color preview' });
    const long = screen.getByRole('button', { name: 'Build RGB 300 s color preview' });
    expect(short).toBeEnabled();
    expect(long).toBeEnabled();
    expect(cardFor(short)).not.toBe(cardFor(long));
    expect(within(cardFor(short)).queryByRole('combobox', { name: /source stack/ })).not.toBeInTheDocument();
    expect(within(cardFor(long)).queryByRole('combobox', { name: /source stack/ })).not.toBeInTheDocument();
    expect(screen.getByText('Custom combination').closest('details')).not.toHaveAttribute('open');
  });

  it.each([30, 300])('submits exact source references from the %i s card', async (seconds) => {
    const { submissions } = mount();
    await userEvent.click(await screen.findByRole('button', { name: `Build RGB ${seconds} s color preview` }));
    await waitFor(() => expect(submissions[0]?.input_sources).toEqual(references(atSeconds(seconds))));
    const active = screen.getByRole('button', { name: `Build RGB ${seconds} s color preview` });
    expect(within(cardFor(active)).getByRole('status')).toHaveTextContent('Waiting');
    expect(screen.getByRole('button', { name: `Build RGB ${seconds === 30 ? 300 : 30} s color preview` })).toBeEnabled();
  });

  it('disables only the incomplete long set when its blue stack is missing', async () => {
    mount({ sources: candidates.filter((source) => source.job_id !== 'blue-300') });
    const short = await screen.findByRole('button', { name: 'Build RGB 30 s color preview' });
    const long = screen.getByRole('button', { name: 'Build RGB 300 s color preview' });
    expect(short).toBeEnabled();
    expect(long).toBeDisabled();
    expect(within(cardFor(long)).getByText('Missing B')).toBeVisible();
    expect(within(cardFor(short)).queryByText('Missing B')).not.toBeInTheDocument();
  });

  it('keeps duplicate same-role sources ambiguous until a custom choice is made', async () => {
    const duplicate = { ...atSeconds(30)[0], filter_name: 'Other red', job_id: 'other-red',
      exposure_group: { ...atSeconds(30)[0].exposure_group!, key: 'other-red-family' } };
    const { submissions } = mount({ sources: [...candidates, duplicate] });
    const short = await screen.findByRole('button', { name: 'Build RGB 30 s color preview' });
    expect(short).toBeDisabled();
    expect(within(cardFor(short)).getByText('Multiple R stacks')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Build RGB 300 s color preview' })).toBeEnabled();
    await userEvent.click(screen.getByText('Custom combination'));
    for (const source of atSeconds(30)) await userEvent.selectOptions(selector(source.role), colorSourceKey(source));
    await userEvent.click(screen.getByRole('button', { name: 'Build RGB custom color preview' }));
    await waitFor(() => expect(submissions[0]?.input_sources).toEqual(references(atSeconds(30))));
  });

  it('allows deliberate mixed exposure sources only through Custom combination', async () => {
    const { submissions } = mount();
    await userEvent.click(await screen.findByText('Custom combination'));
    const mixed = [atSeconds(30)[0], ...atSeconds(300).slice(1)];
    for (const source of mixed) await userEvent.selectOptions(selector(source.role), colorSourceKey(source));
    await userEvent.click(screen.getByRole('button', { name: 'Build RGB custom color preview' }));
    await waitFor(() => expect(submissions[0]?.input_sources).toEqual(references(mixed)));
    expect(screen.getByRole('button', { name: 'Build RGB 30 s color preview' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Build RGB 300 s color preview' })).toBeEnabled();
  });

  it('keeps saved short and long artifacts independently inspectable', async () => {
    mount({ jobs: [job(atSeconds(30), { job_id: 'color-short' }), job(atSeconds(300), { job_id: 'color-long' })] });
    const short = await screen.findByRole('button', { name: 'Inspect RGB 30 s full size' });
    const long = screen.getByRole('button', { name: 'Inspect RGB 300 s full size' });
    expect(within(cardFor(short)).getByRole('img')).toHaveAttribute('src', expect.stringContaining('color-short'));
    expect(within(cardFor(long)).getByRole('img')).toHaveAttribute('src', expect.stringContaining('color-long'));
    await userEvent.click(long);
    expect(within(screen.getByRole('dialog')).getByText('red (300 s), green (300 s), blue (300 s)')).toBeVisible();
  });

  it('scopes a failed queued build to its exposure card', async () => {
    const { submissions } = mount({ response: () => ({ state: 'failed', phase: 'Failed', error: 'Long channel registration failed' }) });
    await userEvent.click(await screen.findByRole('button', { name: 'Build RGB 300 s color preview' }));
    await waitFor(() => expect(submissions).toHaveLength(1));
    const long = cardFor(screen.getByRole('button', { name: 'Build RGB 300 s color preview' }));
    await waitFor(() => expect(within(long).getByText('Long channel registration failed')).toBeVisible());
    const short = cardFor(screen.getByRole('button', { name: 'Build RGB 30 s color preview' }));
    expect(within(short).queryByText('Long channel registration failed')).not.toBeInTheDocument();
    expect(within(short).getByRole('status')).toHaveTextContent('Not built');
  });

  it('retains separate historical-only previews without enabling builds from missing sources', async () => {
    mount({ omitTarget: true, sources: [], jobs: [
      job(atSeconds(30), { job_id: 'history-short', outdated: true }),
      job(atSeconds(300), { job_id: 'history-long', outdated: true }),
    ] });
    const short = cardFor(await screen.findByRole('button', { name: 'Inspect RGB Previous 30 s full size' }));
    const long = cardFor(screen.getByRole('button', { name: 'Inspect RGB Previous 300 s full size' }));
    expect(within(short).getByRole('img')).toHaveAttribute('src', expect.stringContaining('history-short'));
    expect(within(long).getByRole('img')).toHaveAttribute('src', expect.stringContaining('history-long'));
    expect(within(short).getByRole('button', { name: 'Rebuild RGB Previous 30 s color preview' })).toBeDisabled();
    expect(within(long).getByRole('button', { name: 'Rebuild RGB Previous 300 s color preview' })).toBeDisabled();
    for (const build of screen.getAllByRole('button', { name: /^(Build|Rebuild) RGB .*color preview$/ })) {
      expect(build).toBeDisabled();
    }
  });

  it('isolates saved processing and crop changes between exposure cards', async () => {
    const shortProcessing: StackColorProcessing = { background_extraction: null, input_deconvolutions: {}, input_stretches: {},
      output_stretches: [{ model: { type: 'identity' }, color_strategy: 'linked' }] };
    const longProcessing: StackColorProcessing = { ...shortProcessing, output_stretches: [] };
    const { submissions } = mount({ jobs: [
      job(atSeconds(30), { job_id: 'color-short', crop: 'bounds', processing: shortProcessing }),
      job(atSeconds(300), { job_id: 'color-long', crop: 'inscribed', processing: longProcessing }),
    ] });
    const shortCrop = await screen.findByRole('combobox', { name: 'M31 RGB 30 s edge crop' });
    const longCrop = screen.getByRole('combobox', { name: 'M31 RGB 300 s edge crop' });
    expect(shortCrop).toHaveValue('bounds');
    expect(longCrop).toHaveValue('inscribed');
    expect(screen.getByRole('button', { name: 'Apply processing M31 RGB 30 s' })).toHaveAttribute('data-applied', JSON.stringify(shortProcessing));
    expect(screen.getByRole('button', { name: 'Apply processing M31 RGB 300 s' })).toHaveAttribute('data-applied', JSON.stringify(longProcessing));
    await userEvent.selectOptions(shortCrop, 'none');
    expect(longCrop).toHaveValue('inscribed');
    await userEvent.click(screen.getByRole('button', { name: 'Apply processing M31 RGB 300 s' }));
    await waitFor(() => expect(submissions[0]?.input_sources).toEqual(references(atSeconds(300))));
    expect(submissions[0].crop).toBe('inscribed');
    expect(submissions[0].processing.output_stretches).toEqual([{ model: { type: 'linear', black: 0, white: 0.7 }, color_strategy: 'linked' }]);
    expect(shortCrop).toHaveValue('none');
  });

  it('marks only the card containing an exact stale source reference', async () => {
    const longRed = atSeconds(300)[0];
    const { rerender } = mount({ outdated: new Set([colorSourceKey({ ...longRed, artifact_revision: 'other-revision' })]) });
    const short = cardFor(await screen.findByRole('button', { name: 'Build RGB 30 s color preview' }));
    const long = cardFor(screen.getByRole('button', { name: 'Build RGB 300 s color preview' }));
    expect(short).not.toHaveClass('outdated');
    expect(long).not.toHaveClass('outdated');
    rerender('current', new Set([colorSourceKey(longRed)]));
    await waitFor(() => expect(cardFor(screen.getByRole('button', { name: 'Build RGB 300 s color preview' }))).toHaveClass('outdated'));
    expect(cardFor(screen.getByRole('button', { name: 'Build RGB 30 s color preview' }))).not.toHaveClass('outdated');
  });

  it.each([false, true])('preserves unsplit single-source behavior (legacy catalog: %s)', async (legacy) => {
    const unsplit = atSeconds(30).map((source) => ({ ...source, exposure_group: null }));
    const { submissions } = mount({ sources: unsplit, legacy });
    const build = await screen.findByRole('button', { name: 'Build RGB color preview' });
    expect(build).toBeEnabled();
    expect(screen.queryByText('Custom combination')).not.toBeInTheDocument();
    expect(screen.queryByRole('combobox', { name: /source stack/ })).not.toBeInTheDocument();
    await userEvent.click(build);
    await waitFor(() => expect(submissions).toHaveLength(1));
    expect(submissions[0].input_sources).toBeUndefined();
  });

  it('keeps custom stale-source selectors recoverable after grouping is disabled', async () => {
    const { catalog, rerender } = mount();
    await userEvent.click(await screen.findByText('Custom combination'));
    for (const source of atSeconds(300)) await userEvent.selectOptions(selector(source.role), colorSourceKey(source));
    expect(screen.getByRole('button', { name: 'Build RGB custom color preview' })).toBeEnabled();
    catalog.sources = atSeconds(30).map((source) => ({ ...source, job_id: `${source.role}-unsplit`, exposure_group: null }));
    rerender('unsplit');
    await waitFor(() => expect(selector('red')).toHaveValue(''));
    const build = screen.getByRole('button', { name: 'Build RGB color preview' });
    expect(build).toBeDisabled();
    for (const source of catalog.sources) {
      expect(selector(source.role)).toBeVisible();
      await userEvent.selectOptions(selector(source.role), colorSourceKey(source));
    }
    expect(build).toBeEnabled();
  });
});
