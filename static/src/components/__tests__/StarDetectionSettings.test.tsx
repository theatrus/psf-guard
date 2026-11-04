import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import type {
  ProfileRecordView,
  RigStarDetection,
  StarDetectionSettings as Settings,
} from '../../api/types';
import { server } from '../../test/msw-server';
import StarDetectionSettings from '../StarDetectionSettings';

const NINA_DEFAULTS: RigStarDetection = {
  detector: 'auto',
  sensitivity: 'high',
  noise_reduction: 'none',
  hocus_focus_version: 'v4',
};
const ASKAR = '5a1e0000-0000-5000-8000-0000000000a1';
const C925 = '5a1e0000-0000-5000-8000-0000000000c9';

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

/** Serve `initial`, and record every write the form sends. */
function serve(initial: Settings) {
  let current = initial;
  const writes: { method: string; url: string; body: unknown }[] = [];
  const reply = () => HttpResponse.json({ success: true, error: null, data: current });
  server.use(
    http.get('/api/settings/star-detection', reply),
    http.put('/api/settings/star-detection', async ({ request }) => {
      const body = (await request.json()) as { default: RigStarDetection };
      writes.push({ method: 'PUT', url: request.url, body });
      current = { ...current, default: body.default };
      return reply();
    }),
    http.put('/api/settings/star-detection/rigs/:uuid', async ({ request, params }) => {
      const body = (await request.json()) as { settings: RigStarDetection };
      writes.push({ method: 'PUT', url: request.url, body });
      current = {
        ...current,
        rigs: current.rigs.map((rig) => rig.rig_uuid === params.uuid ? { ...rig, settings: body.settings } : rig),
      };
      return reply();
    }),
    http.put('/api/settings/star-detection/profiles/:id/manual', async ({ request, params }) => {
      const body = (await request.json()) as { settings: RigStarDetection };
      writes.push({ method: 'PUT', url: request.url, body });
      current = {
        ...current,
        profiles: current.profiles.map((profile) =>
          profile.profile_id === params.id ? { ...profile, manual: body.settings } : profile),
      };
      return reply();
    }),
    http.delete('/api/settings/star-detection/profiles/:id/records/:record', ({ request, params }) => {
      writes.push({ method: 'DELETE', url: request.url, body: null });
      current = {
        ...current,
        profiles: current.profiles.map((profile) => profile.profile_id === params.id
          ? { ...profile, records: profile.records.filter((record) => record.id !== params.record) }
          : profile),
      };
      return reply();
    }),
    http.post('/api/settings/star-detection/profiles/import', async ({ request }) => {
      const url = new URL(request.url);
      const body = await request.text();
      writes.push({ method: 'POST', url: request.url, body });
      const preview = url.searchParams.get('preview') === 'true';
      return HttpResponse.json({
        success: true,
        error: null,
        data: {
          profile_id: REDCAT_PROFILE,
          name: 'starfront-mini-redcat61',
          record: redcatRecord('imported', url.searchParams.get('effective_from') ?? undefined),
          warnings: [],
          stored: !preview,
          settings: current,
        },
      });
    }),
    http.delete('/api/settings/star-detection/rigs/:uuid', ({ request, params }) => {
      writes.push({ method: 'DELETE', url: request.url, body: null });
      current = {
        ...current,
        rigs: current.rigs.map((rig) => rig.rig_uuid === params.uuid ? { ...rig, settings: undefined } : rig),
      };
      return reply();
    }),
  );
  return writes;
}

const REDCAT_PROFILE = '46d7a6e3-3ae8-48a9-8d29-598e3647f131';

function redcatRecord(id: string, effectiveFrom?: string, crop = 0.7): ProfileRecordView {
  return {
    id,
    effective_from: effectiveFrom,
    source: 'import',
    received_at: '2026-11-02T12:00:00Z',
    hocus_focus_version: 'v4',
    settings: {
      star_detector: 'hocus_focus',
      star_sensitivity: 'high',
      noise_reduction: 'none',
      auto_stretch_factor: 0.2,
      black_clipping: -2.8,
      autofocus_inner_crop_ratio: crop,
      autofocus_outer_crop_ratio: 1,
      autofocus_brightest_stars: 0,
      hocus_focus: { Simple_PixelScale: 'WideField' },
    },
    hocus_focus_mode: 'simple',
    hocus_focus: {
      hotpixel_filtering: true,
      hotpixel_thresholding: true,
      hotpixel_threshold: 0.001,
      noise_reduction_radius: 4,
      measurement_noise_reduction: false,
      noise_clipping_multiplier: 4,
      star_clipping_multiplier: 2,
      structure_layers: 3,
      brightness_sensitivity: 10,
      star_peak_response: 0.75,
      max_distortion: 0.5,
      star_center_tolerance: 0.3,
      background_box_expansion: 3,
      min_star_bounding_box: 4,
      min_hfr: 1.2,
      pixel_sample_size: 0.5,
      saturation_threshold: 0.99,
      exclude_saturated_from_hfr: true,
      measurement_average: 'median',
      adaptive_binarization: true,
      adaptive_block_size: 128,
      contamination_sensitivity: 5,
      detection_binning: 1,
      autofocus_inner_crop_ratio: crop,
      autofocus_outer_crop_ratio: 1,
      autofocus_brightest_stars: 0,
    },
    notes: [],
  };
}

const settings = (): Settings => ({
  default: NINA_DEFAULTS,
  builtin: NINA_DEFAULTS,
  rigs: [
    {
      rig_uuid: C925,
      name: 'C925 · ASI6200MM',
      detail: '9576×6388 · bin 1×1',
      databases: ['C925'],
      settings: { ...NINA_DEFAULTS, detector: 'hocus_focus', hocus_focus_version: 'v3' },
    },
    { rig_uuid: ASKAR, name: 'Askar 107 · ASI2600MM', detail: '6248×4176 · bin 1×1', databases: ['Askar', 'Remote'] },
  ],
  profiles: [
    {
      profile_id: REDCAT_PROFILE,
      name: 'starfront-mini-redcat61',
      databases: ['Redcat'],
      frames: 1927,
      records: [
        redcatRecord('sep9', '2026-09-09T00:00:00Z', 0.7),
        redcatRecord('sep1', '2026-09-01T00:00:00Z', 0.5),
      ],
    },
  ],
});

describe('Settings › Review › Star detection', () => {
  it('lists the default and each rig with its own settings or the default', async () => {
    serve(settings());
    render(<StarDetectionSettings />, { wrapper: wrapper() });
    const c925 = await screen.findByRole('group', { name: 'C925 · ASI6200MM star detection' });
    expect(within(c925).getByText('9576×6388 · bin 1×1 · C925')).toBeInTheDocument();
    expect(within(c925).getByRole('checkbox', { name: 'Use the default' })).not.toBeChecked();
    expect(within(c925).getByRole('combobox', { name: 'Detector' })).toHaveValue('hocus_focus');
    expect(within(c925).getByRole('combobox', { name: 'Hocus Focus version' })).toHaveValue('v3');
    // Sensitivity tunes N.I.N.A.'s detector, which this rig does not use.
    expect(within(c925).getByRole('combobox', { name: 'Sensitivity' })).toBeDisabled();

    const askar = screen.getByRole('group', { name: 'Askar 107 · ASI2600MM star detection' });
    expect(within(askar).getByText('6248×4176 · bin 1×1 · Askar, Remote')).toBeInTheDocument();
    expect(within(askar).getByRole('checkbox', { name: 'Use the default' })).toBeChecked();
    expect(within(askar).getByRole('combobox', { name: 'Sensitivity' })).toHaveValue('high');
    expect(within(askar).getByRole('combobox', { name: 'Sensitivity' })).toBeDisabled();
    expect(within(askar).getByRole('combobox', { name: 'Detector' })).toHaveAccessibleDescription(
      /^Auto: Hocus Focus where N\.I\.N\.A\. recorded an FWHM/,
    );
    expect(screen.getByRole('button', { name: 'Save star detection' })).toHaveAttribute('aria-disabled', 'true');
  });

  it('saves a rig given its own settings and a rig set back to the default', async () => {
    const user = userEvent.setup();
    const writes = serve(settings());
    render(<StarDetectionSettings />, { wrapper: wrapper() });
    const askar = await screen.findByRole('group', { name: 'Askar 107 · ASI2600MM star detection' });
    await user.click(within(askar).getByRole('checkbox', { name: 'Use the default' }));
    await user.selectOptions(within(askar).getByRole('combobox', { name: 'Sensitivity' }), 'normal');
    const c925 = screen.getByRole('group', { name: 'C925 · ASI6200MM star detection' });
    await user.click(within(c925).getByRole('checkbox', { name: 'Use the default' }));
    const saveButton = screen.getByRole('button', { name: 'Save star detection' });
    expect(saveButton).toHaveAttribute('aria-disabled', 'false');
    await user.click(saveButton);

    await waitFor(() => expect(writes).toHaveLength(2));
    expect(writes[0]).toMatchObject({ method: 'DELETE' });
    expect(writes[0].url).toContain(`/rigs/${C925}`);
    expect(writes[1]).toMatchObject({
      method: 'PUT',
      body: { name: 'Askar 107 · ASI2600MM', settings: { ...NINA_DEFAULTS, sensitivity: 'normal' } },
    });
    await waitFor(() => expect(saveButton).toHaveAttribute('aria-disabled', 'true'));
    expect(within(askar).getByRole('combobox', { name: 'Sensitivity' })).toHaveValue('normal');
  });

  it('saves the default row alone when only it changed', async () => {
    const user = userEvent.setup();
    const writes = serve(settings());
    render(<StarDetectionSettings />, { wrapper: wrapper() });
    const defaults = await screen.findByRole('group', { name: 'Default' });
    await user.selectOptions(within(defaults).getByRole('combobox', { name: 'Detector' }), 'nina');
    expect(within(defaults).getByRole('combobox', { name: 'Hocus Focus version' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Save star detection' }));
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0]).toMatchObject({ method: 'PUT', body: { default: { ...NINA_DEFAULTS, detector: 'nina' } } });
    expect(writes[0].url).toMatch(/\/settings\/star-detection$/);
  });
});

describe('Settings › Review › Star detection › N.I.N.A. profiles', () => {
  it('shows a profile\'s recorded values, their history, and deletes a record', async () => {
    const user = userEvent.setup();
    const writes = serve(settings());
    render(<StarDetectionSettings />, { wrapper: wrapper() });
    const redcat = await screen.findByRole('group', { name: 'starfront-mini-redcat61 star detection' });
    expect(within(redcat).getByText('1,927 frames · Redcat')).toBeInTheDocument();
    expect(within(redcat).getByText(
      'Hocus Focus 4 · Simple · crop 0.7 · layers 3 · sensitivity 10 · min star box 4 · noise radius 4 · pixel sample 0.5',
    )).toBeInTheDocument();
    expect(within(redcat).getByRole('checkbox', { name: 'Set by hand' })).not.toBeChecked();
    expect(within(redcat).queryByRole('combobox', { name: 'Detector' })).toBeNull();

    await user.click(within(redcat).getByText('History (2)'));
    const remove = within(redcat).getAllByRole('button', { name: /^Delete the record for frames / });
    expect(remove).toHaveLength(2);
    await user.click(remove[1]);
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0].url).toContain(`/profiles/${REDCAT_PROFILE}/records/sep1`);
    await waitFor(() => expect(within(redcat).queryByText('History (2)')).toBeNull());
  });

  it('says which plugin sent a record', async () => {
    const user = userEvent.setup();
    const sent = settings();
    sent.profiles[0].records[0] = { ...sent.profiles[0].records[0], source: 'plugin:director' };
    sent.profiles[0].records[1] = { ...sent.profiles[0].records[1], source: 'plugin:sync' };
    serve(sent);
    render(<StarDetectionSettings />, { wrapper: wrapper() });
    const redcat = await screen.findByRole('group', { name: 'starfront-mini-redcat61 star detection' });
    await user.click(within(redcat).getByText('History (2)'));
    expect(within(redcat).getAllByText(/Sent by the Director plugin/).length).toBeGreaterThan(0);
    expect(within(redcat).getByText(/Sent by the Sync plugin/)).toBeInTheDocument();
  });

  it('says which existing records still apply after an import for all frames', async () => {
    const user = userEvent.setup();
    serve(settings());
    render(<StarDetectionSettings />, { wrapper: wrapper() });
    await screen.findByRole('heading', { name: 'Import N.I.N.A. profile' });
    const file = new File(['<Profile/>'], 'redcat.profile', { type: 'application/xml' });
    await user.upload(screen.getByLabelText('Profile file'), file);
    await user.click(screen.getByRole('button', { name: 'Read file' }));
    const read = await screen.findByRole('status');
    const still = within(read).getAllByText(/^Imported record still applies from .+$/);
    expect(still).toHaveLength(2);
  });

  it('saves a profile set by hand', async () => {
    const user = userEvent.setup();
    const writes = serve(settings());
    render(<StarDetectionSettings />, { wrapper: wrapper() });
    const redcat = await screen.findByRole('group', { name: 'starfront-mini-redcat61 star detection' });
    await user.click(within(redcat).getByRole('checkbox', { name: 'Set by hand' }));
    await user.selectOptions(within(redcat).getByRole('combobox', { name: 'Detector' }), 'nina');
    await user.click(screen.getByRole('button', { name: 'Save star detection' }));
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0]).toMatchObject({
      method: 'PUT',
      body: { name: 'starfront-mini-redcat61', settings: { ...NINA_DEFAULTS, detector: 'nina' } },
    });
    expect(writes[0].url).toContain(`/profiles/${REDCAT_PROFILE}/manual`);
  });

  it('reads a profile file, shows it, then imports it from a date', async () => {
    const user = userEvent.setup();
    const writes = serve(settings());
    render(<StarDetectionSettings />, { wrapper: wrapper() });
    await screen.findByRole('heading', { name: 'Import N.I.N.A. profile' });
    const read = screen.getByRole('button', { name: 'Read file' });
    expect(read).toHaveAttribute('aria-disabled', 'true');
    const file = new File(['<Profile/>'], 'redcat.profile', { type: 'application/xml' });
    await user.upload(screen.getByLabelText('Profile file'), file);
    await user.click(screen.getByRole('radio', { name: 'Frames from' }));
    expect(read).toHaveAttribute('aria-disabled', 'true');
    await user.type(screen.getByLabelText('First day'), '2026-09-09');
    await user.click(read);
    await waitFor(() => expect(writes).toHaveLength(1));
    const preview = new URL(writes[0].url);
    expect(preview.searchParams.get('preview')).toBe('true');
    expect(preview.searchParams.get('hocus_focus_version')).toBe('v4');
    expect(preview.searchParams.get('effective_from')).toBe(new Date('2026-09-09T00:00:00').toISOString());
    expect(writes[0].body).toBe('<Profile/>');
    expect(await screen.findByRole('status')).toHaveTextContent(/^starfront-mini-redcat61 46d7a6e3/);

    await user.click(screen.getByRole('button', { name: 'Import profile' }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect(new URL(writes[1].url).searchParams.get('preview')).toBe('false');
    expect(await screen.findByText(/^Imported starfront-mini-redcat61 · /)).toBeInTheDocument();
  });
});
