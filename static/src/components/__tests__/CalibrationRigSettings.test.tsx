import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import type { CalibrationRigSettings as RigSettings } from '../../api/types';
import { server } from '../../test/msw-server';
import { CALIBRATION_RULE_FIELDS } from '../../test/calibrationRuleFields';
import CalibrationRigSettings from '../CalibrationRigSettings';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const ASKAR = '5a1e0000-0000-5000-8000-0000000000a1';
const REDCAT = '5a1e0000-0000-5000-8000-0000000000b2';

function rig(uuid: string, name: string, rules: RigSettings['rules'] = {}): RigSettings {
  return { rig_uuid: uuid, name, detail: '6248×4176 · bin 1×1', databases: ['Observatory'], frames: 40, rules };
}

const current = (rigs: RigSettings[], global: Record<string, unknown> = { thin_master_frames: 4 }) => ({
  success: true,
  data: {
    rotation_tolerance_deg: null,
    default_rotation_tolerance_deg: 2,
    external_masters: 'prefer',
    flat_star_masking: false,
    fields: CALIBRATION_RULE_FIELDS,
    rules: global,
    rigs,
  },
  error: null,
});

describe('CalibrationRigSettings', () => {
  it('lists each rig with how many values it sets, and shows what it inherits from where', async () => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json(current([
      rig(ASKAR, 'Askar 120 · ASI2600MM', { dark_reach_days: 400, check_no_flat: 'advisory' }),
      rig(REDCAT, 'RedCat 51 · ASI533MC'),
    ]))));
    render(<CalibrationRigSettings />, { wrapper: wrapper() });
    const askar = await screen.findByRole('button', { name: /Askar 120 · ASI2600MM/ });
    expect(askar).toHaveTextContent('2 own values');
    expect(askar).toHaveTextContent('6248×4176 · bin 1×1 · Observatory · 40 calibration frames');
    expect(screen.getByRole('button', { name: /RedCat 51/ })).toHaveTextContent('Inherits all');
    expect(askar).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(askar);
    expect(askar).toHaveAttribute('aria-expanded', 'true');
    const editor = screen.getByRole('region', { name: 'Askar 120 · ASI2600MM calibration settings' });
    // Its own values are set and editable.
    expect(within(editor).getByLabelText('Dark reach')).toHaveValue(400);
    expect(within(editor).getByRole('button', { name: 'Dark reach: own value' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(editor).getByLabelText('No matching flat')).toHaveValue('advisory');
    // The rest inherit, and say from where.
    const thin = within(editor).getByRole('button', { name: 'Thin master below: own value' });
    expect(thin).toHaveAttribute('aria-pressed', 'false');
    expect(thin.closest('.calibration-rule-row')).toHaveTextContent('4 frames · global');
    expect(
      within(editor).getByRole('button', { name: 'Flat age limit: own value' }).closest('.calibration-rule-row'),
    ).toHaveTextContent('60 days · default');
  });

  it('sets a value for one rig, starting from the one it inherits, and saves only its own', async () => {
    let saved: { name?: string; rules: Record<string, unknown> } | null = null;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current([rig(ASKAR, 'Askar 120 · ASI2600MM')]))),
      http.put(`/api/settings/calibration/rigs/${ASKAR}`, async ({ request }) => {
        saved = (await request.json()) as typeof saved;
        return HttpResponse.json(current([rig(ASKAR, 'Askar 120 · ASI2600MM', { thin_master_frames: 3 })]));
      }),
    );
    render(<CalibrationRigSettings />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: /Askar 120/ }));
    const editor = screen.getByRole('region', { name: /calibration settings/ });
    const save = within(editor).getByRole('button', { name: 'Save rig' });
    expect(save).toBeDisabled();
    fireEvent.click(within(editor).getByRole('button', { name: 'Thin master below: own value' }));
    const thin = within(editor).getByLabelText('Thin master below');
    expect(thin).toHaveValue(4);
    fireEvent.change(thin, { target: { value: '' } });
    expect(within(editor).getByText('Enter a value, or let it inherit.')).toBeInTheDocument();
    expect(save).toBeDisabled();
    fireEvent.change(thin, { target: { value: '3' } });
    fireEvent.click(save);
    await waitFor(() => expect(saved).toEqual({ name: 'Askar 120 · ASI2600MM', rules: { thin_master_frames: 3 } }));
    await waitFor(() => expect(screen.getByRole('button', { name: /Askar 120/ })).toHaveTextContent('1 own value'));
    // Letting it inherit again takes the field away.
    fireEvent.click(within(editor).getByRole('button', { name: 'Thin master below: own value' }));
    expect(within(editor).queryByLabelText('Thin master below')).not.toBeInTheDocument();
    expect(within(editor).getByRole('button', { name: 'Save rig' })).toBeEnabled();
  });

  it('starts from a preset, which only fills the form', async () => {
    let saved: { rules: Record<string, unknown> } | null = null;
    server.use(
      http.get('/api/settings/calibration', () => HttpResponse.json(current([rig(ASKAR, 'Askar 120 · ASI2600MM')]))),
      http.put(`/api/settings/calibration/rigs/${ASKAR}`, async ({ request }) => {
        saved = (await request.json()) as typeof saved;
        return HttpResponse.json(current([rig(ASKAR, 'Askar 120 · ASI2600MM', { use_darks: false })]));
      }),
    );
    render(<CalibrationRigSettings />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: /Askar 120/ }));
    const editor = screen.getByRole('region', { name: /calibration settings/ });
    const noDarks = within(editor).getByRole('button', { name: 'No darks' });
    fireEvent.click(noDarks);
    expect(noDarks).toHaveAttribute('aria-pressed', 'true');
    expect(within(editor).getByRole('status')).toHaveTextContent(/darks are left out/);
    expect(within(editor).getByLabelText('Use darks')).toHaveValue('off');
    expect(saved).toBeNull();
    fireEvent.click(within(editor).getByRole('button', { name: 'Save rig' }));
    await waitFor(() => expect(saved?.rules).toEqual({ use_darks: false }));
  });

  it('lets a rig inherit everything again', async () => {
    let deleted = false;
    server.use(
      http.get('/api/settings/calibration', () =>
        HttpResponse.json(current([rig(ASKAR, 'Askar 120 · ASI2600MM', { dark_reach_days: 400 })]))),
      http.delete(`/api/settings/calibration/rigs/${ASKAR}`, () => {
        deleted = true;
        return HttpResponse.json(current([rig(ASKAR, 'Askar 120 · ASI2600MM')]));
      }),
    );
    render(<CalibrationRigSettings />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: /Askar 120/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Inherit everything' }));
    await waitFor(() => expect(deleted).toBe(true));
    await waitFor(() => expect(screen.getByRole('button', { name: /Askar 120/ })).toHaveTextContent('Inherits all'));
    expect(screen.getByRole('button', { name: 'Inherit everything' })).toBeDisabled();
  });

  it('opens a rig a light named, even one no catalog lists', async () => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json(current([rig(REDCAT, 'RedCat 51 · ASI533MC')]))));
    render(<CalibrationRigSettings focusRig={{ uuid: ASKAR, name: 'Askar 120 · ASI2600MM' }} />, { wrapper: wrapper() });
    const askar = await screen.findByRole('button', { name: /Askar 120/ });
    expect(askar).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByRole('region', { name: 'Askar 120 · ASI2600MM calibration settings' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /RedCat 51/ })).toHaveAttribute('aria-expanded', 'false');
  });
});
