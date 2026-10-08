import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import { AccessContext } from '../../../auth/access';
import CollaborationConnections from '../CollaborationConnections';
import type { CollaborationConnection } from '../../../api/collaborationTypes';

const rig = '00000000-0000-4000-8000-000000000001';
const id = '00000000-0000-4000-8000-000000000002';
const ok = (data: unknown) => HttpResponse.json({ success: true, data });
function initial(): CollaborationConnection {
  return { binding: { id, rig_id: rig, base_url: 'https://collaboration.example/', name: 'Rig', agent_id: null, allow_loopback_http: false, state: 'new' }, status: 'not_connected' };
}
function setup(connection: CollaborationConnection, canWrite = true) {
  server.use(http.get(`/api/director/v1/rigs/${rig}/collaboration`, () => ok([connection])));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AccessContext.Provider value={{ canWrite, canCompute: canWrite, logout: async () => undefined, status: { authentication_required: true, authenticated: true, role: canWrite ? 'read_write' : 'read_only', can_compute: canWrite } }}><CollaborationConnections rig={rig} name="Rig" /></AccessContext.Provider></QueryClientProvider>);
  return client;
}
describe('CollaborationConnections', () => {
  it('keeps automation controls available after credential loss', async () => {
    const connection = initial();
    connection.status = 'credential_missing'; connection.binding.agent_id = '000000000001'; connection.binding.state = 'registered';
    connection.binding.background = { enabled: true, catalog_id: rig, project_ids: ['000000000002'], interval_minutes: 15, activate: true };
    let saved: unknown;
    server.use(http.post(`/api/director/v1/collaboration/${id}/work`, async ({ request }) => {
      const input = await request.json() as { operation: string; policy?: unknown };
      if (input.operation === 'background_configure') saved = input;
      return ok({ policy: input.policy ?? connection.binding.background, catalogs: [], projects: [], status: { running: false, last_started_ms: null, last_success_ms: null, next_run_ms: null, last_error: null, result: null } });
    }));
    setup(connection);
    await userEvent.click(await screen.findByText('Automatic work requests'));
    await screen.findByText('Automatic refresh paused');
    await userEvent.click(screen.getByLabelText('Pull tonight automatically'));
    await userEvent.click(screen.getByRole('button', { name: 'Save automation' }));
    await waitFor(() => expect(saved).toEqual({ operation: 'background_configure', expected: connection.binding.background, policy: { ...connection.binding.background, enabled: false } }));
    expect(screen.queryByRole('button', { name: "Pull tonight's work" })).not.toBeInTheDocument();
  });
  it('enables pairing after discovery and clears the submitted code', async () => {
    const connection = initial(); const requests: unknown[] = [];
    server.use(http.post(`/api/director/v1/collaboration/${id}/discover`, () => ok({ pairing: true, signin: false })),
      http.post(`/api/director/v1/collaboration/${id}/pair`, async ({ request }) => {
        requests.push(await request.json()); connection.binding.agent_id = '000000000001'; connection.binding.state = 'registered'; connection.status = 'registered'; return ok(connection);
      }));
    setup(connection);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect' }));
    const input = await screen.findByLabelText('Pairing code');
    expect(screen.getByRole('button', { name: 'Pair' })).toBeDisabled();
    await userEvent.type(input, 'PAIR-ONCE');
    expect(screen.getByRole('button', { name: 'Pair' })).toBeEnabled();
    await userEvent.click(screen.getByRole('button', { name: 'Pair' }));
    await waitFor(() => expect(requests).toEqual([{ code: 'PAIR-ONCE' }]));
    expect(await screen.findByText('Registered (000000000001)')).toBeInTheDocument();
    expect(screen.queryByDisplayValue('PAIR-ONCE')).not.toBeInTheDocument();
  });
  it('retains browser approval controls after a polling rate limit', async () => {
    server.use(http.post(`/api/director/v1/collaboration/${id}/discover`, () => ok({ pairing: false, signin: true })),
      http.post(`/api/director/v1/collaboration/${id}/signin`, () => ok({ status: 'awaiting_browser', url: 'https://collaboration.example/auth/start?code=TEMP', expires_in: 300 })),
      http.post(`/api/director/v1/collaboration/${id}/poll`, () => HttpResponse.json({ error: 'Wait before checking again' }, { status: 429 })));
    setup(initial());
    await userEvent.click(await screen.findByRole('button', { name: 'Connect' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Browser sign-in' }));
    expect(await screen.findByRole('link', { name: 'Open sign-in' })).toHaveAttribute('rel', 'noopener noreferrer');
    await userEvent.click(screen.getByRole('button', { name: 'Check sign-in' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Wait before checking again');
    expect(screen.getByRole('link', { name: 'Open sign-in' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeEnabled();
  });
  it('restores pending controls on reload and does not offer repair as a new agent', async () => {
    const pending = initial(); pending.status = 'awaiting_browser';
    const client = setup(pending);
    expect(await screen.findByRole('button', { name: 'Check sign-in' })).toBeEnabled();
    pending.binding.agent_id = '000000000001'; pending.binding.state = 'registered'; pending.status = 'credential_missing';
    await client.invalidateQueries({ queryKey: ['collaborationConnections', rig] });
    expect(await screen.findByText(/Credential missing/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Pair' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add connection' })).toBeInTheDocument();
  });
  it('allows a new browser sign-in when approval expires', async () => {
    const connection = initial();
    server.use(http.post(`/api/director/v1/collaboration/${id}/discover`, () => ok({ pairing: false, signin: true })),
      http.post(`/api/director/v1/collaboration/${id}/signin`, () => ok({ status: 'awaiting_browser', url: 'https://collaboration.example/auth/start?code=TEMP', expires_in: 300 })),
      http.post(`/api/director/v1/collaboration/${id}/poll`, () => HttpResponse.json({ error: 'Sign-in expired' }, { status: 409 })));
    setup(connection);
    await userEvent.click(await screen.findByRole('button', { name: 'Connect' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Browser sign-in' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Check sign-in' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Sign-in expired');
    expect(screen.queryByRole('link', { name: 'Open sign-in' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Browser sign-in' })).toBeEnabled();
  });
  it('does not expose write controls to a read-only user', async () => {
    setup(initial(), false);
    expect(await screen.findByText('Not connected')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Add connection' })).not.toBeInTheDocument();
  });
});
