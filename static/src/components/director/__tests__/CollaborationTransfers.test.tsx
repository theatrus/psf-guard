import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import CollaborationTransfers from '../CollaborationTransfers';

function setup() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><CollaborationTransfers connection="connection" selection={{ catalog: 'rig', import_id: 'visit', panel: 0, image_guids: [] }} canWrite active selectionReady={false} /></QueryClientProvider>);
}

describe('Transfer automation policy', () => {
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
