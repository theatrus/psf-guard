import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { HttpResponse, http } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import RemotePeerSync from '../RemotePeerSync';

const local = {
  id: 'local',
  name: 'Review copy',
  db_path: '/tmp/local.sqlite',
  image_dirs: ['/images/local'],
};

const peer = {
  id: 'scope-peer',
  name: 'Telescope',
  base_url: 'https://scope.example:3000',
  catalog_id: null,
  token_configured: true,
};

function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

function withPeers(peers: unknown[]) {
  server.use(
    http.get('/api/peers', () =>
      HttpResponse.json({ success: true, data: peers, error: null, status: 'ok' })
    )
  );
}

describe('RemotePeerSync', () => {
  it('previews before it offers to apply', async () => {
    withPeers([peer]);
    let applied = false;
    server.use(
      http.post('/api/databases/local/sync/remote', async ({ request }) => {
        const body = (await request.json()) as { dry_run: boolean };
        applied = applied || !body.dry_run;
        return HttpResponse.json({
          success: true,
          data: {
            applied: !body.dry_run,
            peer_product: 'PSF Guard 0.5.0',
            peer_catalog: 'scope',
            summary: { acquiredimage_inserted: 4, project_updated: 0 },
          },
          error: null,
          status: 'ok',
        });
      })
    );

    const user = userEvent.setup();
    render(<RemotePeerSync databases={[local]} />, { wrapper: wrapper() });

    // Nothing can be applied until something has been previewed.
    expect(
      screen.queryByRole('button', { name: 'Apply this preview' })
    ).not.toBeInTheDocument();

    await user.click(await screen.findByRole('button', { name: 'Preview changes' }));
    expect(await screen.findByText(/acquiredimage inserted 4/)).toBeInTheDocument();
    // A zero counter would bury the one line that matters.
    expect(screen.queryByText(/project updated 0/)).not.toBeInTheDocument();
    expect(applied).toBe(false);

    await user.click(screen.getByRole('button', { name: 'Apply this preview' }));
    await waitFor(() => expect(applied).toBe(true));
    expect(await screen.findByText(/^Applied:/)).toBeInTheDocument();
  });

  it('drops a preview when the direction changes under it', async () => {
    withPeers([peer]);
    server.use(
      http.post('/api/databases/local/sync/remote', () =>
        HttpResponse.json({
          success: true,
          data: {
            applied: false,
            peer_product: 'PSF Guard 0.5.0',
            peer_catalog: 'scope',
            summary: { acquiredimage_inserted: 4 },
          },
          error: null,
          status: 'ok',
        })
      )
    );

    const user = userEvent.setup();
    render(<RemotePeerSync databases={[local]} />, { wrapper: wrapper() });
    await user.click(await screen.findByRole('button', { name: 'Preview changes' }));
    expect(
      await screen.findByRole('button', { name: 'Apply this preview' })
    ).toBeInTheDocument();

    // The preview describes one direction. Switching must not leave an Apply
    // button that would send something else.
    await user.click(screen.getByRole('button', { name: 'Send grades' }));
    expect(
      screen.queryByRole('button', { name: 'Apply this preview' })
    ).not.toBeInTheDocument();
  });

  it('reports an unreachable peer without failing the page', async () => {
    withPeers([peer]);
    server.use(
      http.post('/api/peers/scope-peer/check', () =>
        HttpResponse.json({
          success: true,
          data: {
            reachable: false,
            product: null,
            product_version: null,
            protocol_version: null,
            catalogs: [],
            capabilities: [],
            error: 'connection refused',
          },
          error: null,
          status: 'ok',
        })
      )
    );

    const user = userEvent.setup();
    render(<RemotePeerSync databases={[local]} />, { wrapper: wrapper() });
    await user.click(await screen.findByRole('button', { name: 'Test connection' }));

    expect(await screen.findByText(/connection refused/)).toBeInTheDocument();
    // Still usable: an unreachable peer is a state, not a crash.
    expect(screen.getByRole('button', { name: 'Preview changes' })).toBeEnabled();
  });

  it('says so when no peer is configured', async () => {
    withPeers([]);
    render(<RemotePeerSync databases={[local]} />, { wrapper: wrapper() });
    expect(await screen.findByText('No peers configured')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Preview changes' })).toBeDisabled();
  });

  it('never asks the server for a key it already holds', async () => {
    // The browser is not given the key, so an edit that does not set one must
    // leave the stored key alone rather than blanking it.
    withPeers([peer]);
    let posted: Record<string, unknown> | null = null;
    server.use(
      http.post('/api/peers', async ({ request }) => {
        posted = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json({
          success: true,
          data: { ...peer, id: 'new-peer', name: 'Second scope' },
          error: null,
          status: 'ok',
        });
      })
    );

    const user = userEvent.setup();
    render(<RemotePeerSync databases={[local]} />, { wrapper: wrapper() });
    await user.click(await screen.findByRole('button', { name: 'Add peer' }));
    await user.type(screen.getByLabelText('Name'), 'Second scope');
    await user.clear(screen.getByLabelText('Base URL'));
    await user.type(screen.getByLabelText('Base URL'), 'https://second.example');
    await user.type(screen.getByLabelText('API key'), 'a-key-long-enough-for-the-peer');
    await user.click(screen.getByRole('button', { name: 'Save peer' }));

    await waitFor(() => expect(posted).not.toBeNull());
    expect(posted).toMatchObject({
      name: 'Second scope',
      base_url: 'https://second.example',
      token: 'a-key-long-enough-for-the-peer',
    });
  });
});
