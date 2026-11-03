import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import type { RigStarDetection, StarDetectionSettings as Settings } from '../../api/types';
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
      /^Detector: Auto uses Hocus Focus where N\.I\.N\.A\. recorded an FWHM/,
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
