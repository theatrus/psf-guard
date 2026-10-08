import { describe, expect, it } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import CollaborationBackground from '../CollaborationBackground';
import type { CollaborationBackgroundReply, CollaborationConnection } from '../../../api/collaborationTypes';

const connection: CollaborationConnection = { status: 'registered', binding: { id: 'connection', rig_id: 'rig', base_url: 'https://collab.example/', name: 'Rig', agent_id: '000000000001', allow_loopback_http: false, state: 'registered' } };
const initial: CollaborationBackgroundReply = { policy: null, catalogs: [{ id: 'catalog', slug: 'rig-db', name: 'RedCat 61' }], projects: [{ id: '000000000002', name: 'M31' }], status: { running: false, last_started_ms: null, last_success_ms: null, next_run_ms: null, last_error: null, result: null } };
function setup(canWrite = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><CollaborationBackground connection={connection} canWrite={canWrite} refresh={() => undefined} /></QueryClientProvider>);
  return client;
}
describe('Automatic collaboration work', () => {
  it('keeps remote rejection reasons visible when the latest pass delivered nothing', async () => {
    const data = { ...initial, policy: { enabled: false, catalog_id: 'catalog', project_ids: [], interval_minutes: 15, activate: false, automatic_reports: true }, status: {
      ...initial.status, reports: { queued: 0, delivered: 0, accepted: 0, rejected: 0, held: 0 },
      rejected_reports: [{ id: 'report', night: '2026-10-07', panel: 2, filter: 'H', reasons: ['Quality evidence incomplete'], summary: null }],
    } };
    server.use(http.post('/api/director/v1/collaboration/connection/work', () => HttpResponse.json({ success: true, data })));
    setup();
    await userEvent.click(screen.getByText('Automatic work requests'));
    expect(await screen.findByRole('alert')).toHaveTextContent('2026-10-07, panel 2, H: Quality evidence incomplete');
    expect(screen.getByText(/0 accepted, 0 rejected/)).toBeVisible();
  });
  it('can submit reports without enabling nightly intake or allowing new projects', async () => {
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const input = await request.json() as Record<string, unknown>; requests.push(input);
      return HttpResponse.json({ success: true, data: { ...initial, policy: input.policy ?? null } });
    }));
    setup();
    await userEvent.click(screen.getByText('Automatic work requests'));
    await screen.findByText('Rig database: RedCat 61');
    await userEvent.click(screen.getByLabelText('Submit contribution reports automatically'));
    await userEvent.click(screen.getByRole('button', { name: 'Save automation' }));
    await waitFor(() => expect(requests.at(-1)).toMatchObject({ operation: 'background_configure', policy: { enabled: false, project_ids: [], automatic_reports: true, catalog_id: 'catalog' } }));
    expect(screen.getByRole('button', { name: 'Refresh now' })).toBeEnabled();
  });
  it('is off by default and requires explicit projects and activation consent', async () => {
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const input = await request.json() as Record<string, unknown>; requests.push(input);
      return HttpResponse.json({ success: true, data: { ...initial, policy: input.policy ?? null } });
    }));
    setup();
    expect(requests).toEqual([]);
    await userEvent.click(screen.getByText('Automatic work requests'));
    expect(await screen.findByLabelText('Pull tonight automatically')).not.toBeChecked();
    await screen.findByText('Rig database: RedCat 61');
    expect(screen.queryByRole('combobox', { name: 'Rig database' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Refresh interval (minutes)')).not.toBeInTheDocument();
    expect(screen.getByLabelText('M31')).not.toBeChecked();
    expect(screen.getByLabelText('Activate in rig database')).not.toBeChecked();
    await userEvent.click(screen.getByLabelText('Pull tonight automatically'));
    expect(screen.getByRole('button', { name: 'Save automation' })).toBeDisabled();
    await userEvent.click(screen.getByLabelText('M31'));
    await userEvent.click(screen.getByLabelText('Activate in rig database'));
    await userEvent.click(screen.getByLabelText('Submit contribution reports automatically'));
    await userEvent.click(screen.getByRole('button', { name: 'Save automation' }));
    await waitFor(() => expect(requests.at(-1)).toEqual({ operation: 'background_configure', expected: null, policy: { enabled: true, catalog_id: 'catalog', project_ids: ['000000000002'], interval_minutes: 15, activate: true, automatic_reports: true } }));
  });
  it('retains unsaved edits and their original compare-and-set policy during status polling', async () => {
    let data = structuredClone(initial);
    const requests: Record<string, unknown>[] = [];
    server.use(http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
      const input = await request.json() as Record<string, unknown>; requests.push(input);
      return input.operation === 'background_configure'
        ? HttpResponse.json({ error: 'Background policy changed; reload before saving' }, { status: 409 })
        : HttpResponse.json({ success: true, data });
    }));
    const client = setup();
    await userEvent.click(screen.getByText('Automatic work requests'));
    await screen.findByText('Rig database: RedCat 61');
    await userEvent.click(screen.getByLabelText('Pull tonight automatically'));
    await userEvent.click(screen.getByLabelText('M31'));
    data = { ...data, policy: { enabled: false, catalog_id: 'catalog', project_ids: ['000000000002'], interval_minutes: 60, activate: false, automatic_reports: true } };
    await act(() => client.invalidateQueries({ queryKey: ['collaboration-background', 'connection'] }));
    expect(screen.getByLabelText('Submit contribution reports automatically')).not.toBeChecked();
    await userEvent.click(screen.getByRole('button', { name: 'Save automation' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Background policy changed');
    expect(requests.find(r => r.operation === 'background_configure')?.expected).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Discard automation edits' }));
    await waitFor(() => expect(screen.getByLabelText('Submit contribution reports automatically')).toBeChecked());
  });
  it('shows held work and errors without disabling a recovery edit', async () => {
    const data = { ...initial, policy: { enabled: true, catalog_id: 'catalog', project_ids: ['000000000002'], interval_minutes: 15, activate: true }, status: { ...initial.status, last_error: 'Server offline', result: { night: '2026-10-05', imported: 0, unchanged: 1, activated: 0, held: ['Choose recipes in Planning'] } } };
    server.use(http.post('/api/director/v1/collaboration/connection/work', () => HttpResponse.json({ success: true, data })));
    setup();
    await userEvent.click(screen.getByText('Automatic work requests'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Existing plans retained');
    expect(screen.getByText('Choose recipes in Planning')).toBeVisible();
    await userEvent.click(screen.getByLabelText('Pull tonight automatically'));
    expect(screen.getByRole('button', { name: 'Save automation' })).toBeEnabled();
  });
  it('shows a rejected connection from polled status even while the parent still says registered', async () => {
    const data = { ...initial, connection_status: 'reauth_required', policy: { enabled: true, catalog_id: 'catalog', project_ids: ['000000000002'], interval_minutes: 15, activate: true } };
    server.use(http.post('/api/director/v1/collaboration/connection/work', () => HttpResponse.json({ success: true, data })));
    setup();
    await userEvent.click(screen.getByText('Automatic work requests'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Automatic refresh paused: reauth required');
    expect(screen.getByRole('button', { name: 'Refresh now' })).toBeDisabled();
    expect(screen.getByText(/Next refresh: Paused/)).toBeVisible();
  });
});
