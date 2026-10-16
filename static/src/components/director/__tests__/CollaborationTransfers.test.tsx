import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import CollaborationTransfers from '../CollaborationTransfers';

function setup(selectionReady = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><CollaborationTransfers connection="connection" selection={{ catalog: 'rig', import_id: 'visit', panel: 0, image_guids: selectionReady ? ['image'] : [] }} canWrite active selectionReady={selectionReady} /></QueryClientProvider>);
}

describe('Transfer automation policy', () => {
  it('uploads pending files without a mandatory prepare or approval step', async () => {
    const jobs: Record<string, unknown>[] = [];
    const requests: Record<string, unknown>[] = [];
    server.use(
      http.get('/api/director/v1/transfers', () => HttpResponse.json({ success: true, data: { jobs, policies: [] } })),
      http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
        const input = await request.json() as Record<string, unknown>;
        requests.push(input);
        if (input.operation === 'upload_pending') jobs.push({ id: 'job', connection_id: 'connection', revision: 1, state: 'queued', label: 'M31', total_files: 1, completed_files: 0, total_bytes: 100, completed_bytes: 0, sent_bytes: 0, warnings: [] });
        return HttpResponse.json({ success: true, data: input.operation === 'transfer_outputs' ? { outputs: [] } : { transfer: jobs[0] } });
      }),
    );
    setup(true);
    await userEvent.click(screen.getByRole('button', { name: 'Upload pending' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Pending');
    expect(screen.queryByRole('button', { name: /Prepare|Upload reviewed/ })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Inspect files' })).toBeEnabled();
    expect(requests.filter(r => r.operation === 'upload_pending')).toEqual([{ operation: 'upload_pending', request: { selection: { catalog: 'rig', import_id: 'visit', panel: 0, image_guids: ['image'] }, confirm_wbpp_membership: false } }]);
    expect(requests.some(r => r.operation === 'transfer_control')).toBe(false);
  });

  it('waits for the saved policy and preserves consent when changing file mode', async () => {
    let release!: () => void;
    const ready = new Promise<void>(resolve => { release = resolve; });
    let policy = { connection_id: 'connection', automatic: true, mode: 'psf_guard' };
    const requests: Record<string, unknown>[] = [];
    server.use(
      http.get('/api/director/v1/transfers', async () => {
        await ready;
        return HttpResponse.json({ success: true, data: { jobs: [], policies: [policy] } });
      }),
      http.post('/api/director/v1/collaboration/connection/work', async ({ request }) => {
        const input = await request.json() as typeof policy & { operation: string };
        requests.push(input);
        policy = { ...policy, automatic: input.automatic, mode: input.mode };
        return HttpResponse.json({ success: true, data: { saved: true } });
      }),
    );
    setup();
    const enabled = screen.getByLabelText('Upload accepted collaboration files automatically');
    const mode = screen.getByLabelText('Automatic files');
    expect(enabled).toBeDisabled();
    expect(mode).toBeDisabled();
    release();
    await waitFor(() => expect(enabled).toBeEnabled());
    expect(enabled).toBeChecked();
    expect(mode).toHaveValue('psf_guard');
    await userEvent.selectOptions(mode, 'raw');
    await waitFor(() => expect(requests).toEqual([{ operation: 'transfer_policy', automatic: true, mode: 'raw' }]));
    await waitFor(() => expect(mode).toHaveValue('raw'));
    expect(enabled).toBeChecked();
  });

  it('does not allow unknown policy defaults to overwrite saved settings after a load error', async () => {
    server.use(http.get('/api/director/v1/transfers', () => HttpResponse.json({ error: 'Transfer metadata unavailable' }, { status: 503 })));
    setup();
    expect(await screen.findByRole('alert')).toHaveTextContent('Transfer metadata unavailable');
    expect(screen.getByLabelText('Automatic files')).toBeDisabled();
    expect(screen.getByLabelText('Upload accepted collaboration files automatically')).toBeDisabled();
  });
});
