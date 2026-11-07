import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import RigPluginPanel from '../director/RigPluginPanel';
import type { DirectorPluginClient, DirectorPluginGrant, DirectorPluginView, DirectorRigMode } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const rig = '22222222-2222-4222-8222-222222222222';
const catalog = '33333333-3333-4333-8333-333333333333';
const instance = '44444444-4444-4444-8444-444444444444';
const equipment = { id: 'config-1', camera_id: 'ASI2600', filter_wheel_id: 'wheel', filters: [{ id: 'ha', position: 1 }] };
const plugin: DirectorPluginClient = {
  client_id: 'client-1', profile_id: 'profile-1', client_name: 'NINA observatory', created_at_ms: 1_700_000_000_000, report: null, review: 'none',
};
const reported: DirectorPluginClient = {
  ...plugin,
  review: 'ready',
  report: { report_id: 'report-1', observed_at_ms: Date.now(), received_at_ms: Date.now(), configuration: equipment, filter_names: { ha: 'H-alpha' }, accepted_revision: null },
};
const plans = [{ id: 'plan-1', name: 'Heart Nebula' }, { id: 'plan-2', name: 'M31' }];
const base: DirectorPluginView = {
  coordinator_instance_id: instance, catalog_id: catalog, rig_id: rig, profile_revision: 4, configuration: null, filter_names: {}, camera_defaults: null,
  clients: [], plans: [], policy: null, work: 'not_allowed', left_out: [], grant: null, last_contact_at_ms: null,
};
const policy = {
  rig_id: rig, catalog_id: catalog, client_id: 'client-1', profile_id: 'profile-1', profile_revision: 4, configuration_id: 'config-1',
  project_ids: ['plan-1'], enabled: true, revision: 2,
};
const grant: DirectorPluginGrant = {
  allocation_id: 'work-1', client_id: 'client-1', admitted_at_ms: Date.now(), expires_at_ms: null, started: true, open: true, plugin_paired: true, closed_at_ms: null,
};

function show(view: DirectorPluginView, { mode = 'target_scheduler' as DirectorRigMode, canWrite = true, syncClients = 0 } = {}) {
  let current = view;
  const calls: { path: string; body: unknown }[] = [];
  const record = (path: string) => async ({ request }: { request: Request }) => {
    calls.push({ path, body: request.method === 'DELETE' ? null : await request.json() });
    return HttpResponse.json(ok({ pairing_token: 'psfdpt_code', expires_at_ms: Date.now() + 3_600_000 }));
  };
  server.use(
    http.get('/api/director/v1/catalogs/rig/rig/plugin', () => HttpResponse.json(ok(current))),
    http.get('/api/databases', () => HttpResponse.json(ok([{ id: 'rig', name: 'Rig', remote_image_upload: { clients: Array.from({ length: syncClients }, (_, i) => ({ client_uuid: `${i}`, name: 'Sync', paired_at: 1 })) } }]))),
    http.post(`/api/director/v1/rigs/${rig}/pairing-token`, record('pairing-token')),
    http.delete(`/api/director/v1/rigs/${rig}/clients/client-1`, record('remove')),
    http.post(`/api/director/v1/rigs/${rig}/equipment-reports/client-1/accept`, record('accept')),
    http.put(`/api/director/v1/rigs/${rig}/workload-policy`, record('policy')),
    http.post(`/api/director/v1/rigs/${rig}/workloads/work-1/close`, record('close')),
  );
  const onRigChanged = vi.fn();
  const onChangeMode = vi.fn();
  const access = { ...({} as ReturnType<typeof useAccess>), canWrite };
  render(<AccessContext.Provider value={access}>
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RigPluginPanel slug="rig" mode={mode} onRigChanged={onRigChanged} onChangeMode={onChangeMode} />
    </QueryClientProvider>
  </AccessContext.Provider>);
  return { calls, onRigChanged, onChangeMode, update: (next: DirectorPluginView) => { current = next; } };
}

describe('RigPluginPanel', () => {
  beforeEach(() => { vi.spyOn(window, 'confirm').mockReturnValue(true); });
  afterEach(() => { vi.restoreAllMocks(); });

  it('shows both plugins side by side, whichever picks the work', async () => {
    const { onChangeMode } = show({ ...base, clients: [plugin] }, { syncClients: 2 });
    const paths = (await screen.findByText('Picks the work')).closest('fieldset')!;
    expect(paths).toHaveTextContent('Target Scheduler');
    expect(paths).toHaveTextContent('PSF Guard Director1 paired');
    await waitFor(() => expect(paths).toHaveTextContent('PSF Guard Sync2 paired'));
    fireEvent.click(within(paths).getByRole('button', { name: 'Change mode' }));
    expect(onChangeMode).toHaveBeenCalled();
  });

  it('generates a code for this rig and stops showing it once the plugin pairs', async () => {
    const { calls, update, onRigChanged } = show(base);
    fireEvent.click(await screen.findByRole('button', { name: 'Generate Director pairing code' }));
    const input = await screen.findByLabelText('Director pairing code');
    expect(input).toHaveValue('psfdpt_code');
    expect(calls[0]).toEqual({ path: 'pairing-token', body: { coordinator_instance_id: instance, catalog_id: catalog } });

    update({ ...base, clients: [plugin] });
    await waitFor(() => expect(screen.queryByLabelText('Director pairing code')).toBeNull(), { timeout: 5_000 });
    expect(within(screen.getByRole('list', { name: 'Paired Director plugins' })).getByText(/NINA observatory/)).toBeInTheDocument();
    expect(onRigChanged).toHaveBeenCalled();
  });

  it('accepts a fresh report against the revision it loaded', async () => {
    const { calls, onRigChanged } = show({ ...base, clients: [reported] });
    await screen.findByText(/reported ASI2600/);
    expect(screen.getByText(/Filters: H-alpha/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Accept equipment' }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].body).toEqual({ coordinator_instance_id: instance, catalog_id: catalog, report_id: 'report-1', expected_revision: 4 });
    await waitFor(() => expect(onRigChanged).toHaveBeenCalled());
  });

  it('offers no way to accept an old report or one that open work blocks', async () => {
    show({ ...base, clients: [{ ...reported, review: 'old' }] });
    expect(await screen.findByText(/more than 15 minutes old/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Accept equipment' })).toBeNull();
  });

  it('says what is missing before work can be allowed', async () => {
    show({ ...base, clients: [plugin] });
    expect(await screen.findByText('Accept the equipment first.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Allow work' })).toBeNull();
  });

  it('allows every activated plan, with no list that a later plan could fall outside', async () => {
    const { calls } = show({ ...base, clients: [plugin], configuration: equipment, plans });
    expect(await screen.findByTestId('rig-plugin-plans')).toHaveTextContent('Runs every plan activated on this rig: Heart Nebula, M31.');
    expect(screen.queryByRole('checkbox')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Allow work' }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].body).toEqual({
      coordinator_instance_id: instance, expected_revision: 0,
      policy: { ...policy, profile_revision: 4, project_ids: [], revision: 1 },
    });
  });

  it('can allow work before any plan is activated', async () => {
    show({ ...base, clients: [plugin], configuration: equipment });
    expect(await screen.findByTestId('rig-plugin-plans')).toHaveTextContent('No plans are activated on this rig yet');
    expect(screen.getByRole('button', { name: 'Allow work' })).toBeEnabled();
  });

  it('replaces an out-of-date list of plans when work is allowed again', async () => {
    const { calls } = show({ ...base, clients: [plugin], configuration: equipment, plans, policy, work: 'plans_changed' });
    expect(await screen.findByTestId('rig-plugin-work')).toHaveTextContent('outside the allowed list');
    fireEvent.click(screen.getByRole('button', { name: 'Allow work again' }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].body).toMatchObject({ expected_revision: 2, policy: { project_ids: [], revision: 3 } });
  });

  it('explains a stopped rig and allows work again on the new setup', async () => {
    const { calls } = show({ ...base, clients: [plugin], configuration: equipment, plans, profile_revision: 7, policy, work: 'setup_changed' });
    expect(await screen.findByTestId('rig-plugin-work')).toHaveTextContent("This rig's setup changed");
    fireEvent.click(screen.getByRole('button', { name: 'Allow work again' }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].body).toEqual({ coordinator_instance_id: instance, expected_revision: 2, policy: { ...policy, profile_revision: 7, project_ids: [], revision: 3 } });
  });

  it('says the plugin would get no work, and why, even though work is allowed', async () => {
    const reason = "M31: template 'HA' uses the camera's default gain or offset, and the plugin has not reported them.";
    show({ ...base, clients: [plugin], configuration: equipment, plans, policy, work: 'nothing_to_run', left_out: [reason] });
    expect(await screen.findByTestId('rig-plugin-work')).toHaveTextContent('no plan can run');
    expect(screen.getByRole('alert')).toHaveTextContent('The plugin would get no work');
    expect(within(screen.getByRole('list', { name: 'Left out of the work' })).getByText(reason)).toBeInTheDocument();
    expect(screen.queryByText(/To start, add Director Session/)).not.toBeInTheDocument();
  });

  it('lists goals left out of a rig that still has work, without alarm', async () => {
    show({ ...base, clients: [plugin], configuration: equipment, plans, policy, work: 'ready', left_out: ['M31: 3 finished goals left out'] });
    expect(await screen.findByText('M31: 3 finished goals left out')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('tells a silent plugin apart from a broken one and gives the next step', async () => {
    show({ ...base, clients: [plugin], configuration: equipment, plans, policy, work: 'ready' });
    expect(await screen.findByText('No status yet. The plugin only reports while a Director Session runs.')).toBeInTheDocument();
    expect(screen.getByText(/To start, add Director Session to a sequence in N\.I\.N\.A\./)).toBeInTheDocument();
  });

  it('shows camera defaults for review and flags a change', async () => {
    const report = { ...reported.report!, camera_defaults: { gain: 100, offset: 30 } };
    show({ ...base, clients: [{ ...reported, report }], configuration: equipment, filter_names: { ha: 'H-alpha' }, camera_defaults: { gain: 100, offset: 10 } });
    expect(await screen.findByText(/reported ASI2600.*Camera defaults: gain 100, offset 30\./)).toBeInTheDocument();
    expect(screen.getByText(/Accepted: ASI2600.*Camera defaults: gain 100, offset 10\./)).toBeInTheDocument();
    expect(screen.getByText('Differs from the accepted equipment: camera defaults.')).toBeInTheDocument();
  });

  it('turns work off', async () => {
    const { calls } = show({ ...base, clients: [plugin], configuration: equipment, plans, policy, work: 'ready' });
    fireEvent.click(await screen.findByRole('button', { name: 'Turn off' }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].body).toMatchObject({ expected_revision: 2, policy: { enabled: false, revision: 3 } });
  });

  it('notes that Target Scheduler mode does not stop allowed work', async () => {
    show({ ...base, clients: [plugin], configuration: equipment, plans, policy, work: 'ready' }, { mode: 'target_scheduler' });
    expect(await screen.findByText(/The plugin still gets work when the N.I.N.A. sequence runs Director/)).toBeInTheDocument();
  });

  it('warns before removing a plugin that has open work', async () => {
    const { calls } = show({ ...base, clients: [plugin], grant });
    fireEvent.click(await screen.findByRole('button', { name: 'Remove NINA observatory' }));
    expect(window.confirm).toHaveBeenCalledWith(expect.stringContaining('It has open work'));
    await waitFor(() => expect(calls[0]?.path).toBe('remove'));
  });

  it('keeps a plugin when the removal is cancelled', async () => {
    vi.mocked(window.confirm).mockReturnValue(false);
    const { calls } = show({ ...base, clients: [plugin] });
    fireEvent.click(await screen.findByRole('button', { name: 'Remove NINA observatory' }));
    expect(calls).toHaveLength(0);
  });

  it('offers Close work only once the plugin holding the work is gone', async () => {
    show({ ...base, clients: [plugin], grant });
    await screen.findByText(/^Running\. Issued/);
    expect(screen.queryByRole('button', { name: 'Close work' })).toBeNull();
  });

  it('closes stranded work after a confirmation', async () => {
    const { calls } = show({ ...base, grant: { ...grant, plugin_paired: false } });
    expect(await screen.findByRole('alert')).toHaveTextContent('this rig cannot take new work');
    fireEvent.click(screen.getByRole('button', { name: 'Close work' }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).toEqual({ path: 'close', body: { coordinator_instance_id: instance, catalog_id: catalog } });
  });

  it('asks a read-only session to sign in as an editor', async () => {
    show(base, { canWrite: false });
    expect(await screen.findByText(/Sign in as an editor/)).toBeInTheDocument();
  });
});
