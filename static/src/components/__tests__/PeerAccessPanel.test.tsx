import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import PeerAccessPanel from '../rigs/PeerAccessPanel';

const ok = (data: unknown) => ({ success: true, data, error: null });
const at = Math.floor(Date.now() / 1000) - 5 * 60;
const contact = { at, capabilities: [], places_new_projects: false };
const plugin = { client_uuid: 'a', name: 'Observatory PC', paired_at: at, last_contact: { ...contact, version: '1.4.0.0' } };
const peer = { client_uuid: 'p', name: 'home-server', kind: 'peer', paired_at: at, last_contact: contact };

type Receiver = Record<string, unknown>;
function show(receiver: Receiver, { failSaves = false } = {}) {
  let current: Receiver = {
    enabled: false, sync_enabled: false, peer_sync_enabled: false, token_configured: false, key_configured: false, clients: [], placement: 'flat',
    directory_template: '%TARGET%/%DATE%/%TYPE%', directory_template_source: 'preset', directory_template_samples: 0, ...receiver,
  };
  const saves: Record<string, unknown>[] = [];
  const revoked: string[] = [];
  const codeKinds: (string | null)[] = [];
  server.use(
    http.get('/api/databases', () => HttpResponse.json(ok([{ id: 'remote', name: 'Remote', database_path: '/remote.sqlite', image_directories: [], remote_image_upload: current }]))),
    http.put('/api/databases/remote', async ({ request }) => {
      const body = await request.json() as { remote_image_upload: Record<string, unknown> };
      saves.push(body.remote_image_upload);
      if (failSaves) return HttpResponse.json({ success: false, data: null, error: 'the registry could not be saved' }, { status: 400 });
      const { clear_token, token, ...rest } = body.remote_image_upload;
      current = { ...current, ...Object.fromEntries(Object.entries(rest).filter(([, value]) => value !== undefined)) };
      if (token) current.key_configured = true;
      if (clear_token) current.key_configured = false;
      return HttpResponse.json(ok({ id: 'remote' }));
    }),
    http.post('/api/databases/remote/pairing-token', ({ request }) => {
      codeKinds.push(new URL(request.url).searchParams.get('kind'));
      return HttpResponse.json(ok({ pairing_token: 'psfpt_code', expires_at: Math.floor(Date.now() / 1000) + 3600 }));
    }),
    http.delete('/api/databases/remote/clients/:client', ({ params }) => { revoked.push(String(params.client)); return HttpResponse.json(ok(null)); }),
  );
  const access = { ...({} as ReturnType<typeof useAccess>), canWrite: true };
  render(<AccessContext.Provider value={access}>
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <PeerAccessPanel databaseId="remote" databaseName="Remote" />
    </QueryClientProvider>
  </AccessContext.Provider>);
  return { saves, revoked, codeKinds };
}

describe('Other PSF Guard servers panel', () => {
  it('lists paired servers, not Sync plugins, with their last sync, and removes one', async () => {
    const { revoked } = show({ token_configured: true, peer_sync_enabled: true, clients: [plugin, peer, { client_uuid: 'q', name: 'laptop', kind: 'peer', paired_at: at }] });
    const list = await screen.findByRole('list', { name: 'Paired PSF Guard servers' });
    expect(within(list).getAllByRole('listitem')).toHaveLength(2);
    expect(within(list).getByText(/home-server/).closest('li')).toHaveTextContent('last sync 5 min ago');
    expect(within(list).getByText(/laptop/).closest('li')).toHaveTextContent('no sync recorded');
    expect(screen.queryByText(/Observatory PC/)).toBeNull();
    vi.spyOn(window, 'confirm').mockReturnValueOnce(true);
    fireEvent.click(screen.getByRole('button', { name: 'Remove laptop' }));
    await waitFor(() => expect(revoked).toEqual(['q']));
  });

  it('generates a code for a server, with the steps for the other PSF Guard', async () => {
    const { codeKinds } = show({});
    fireEvent.click(await screen.findByRole('button', { name: 'Generate server pairing code' }));
    expect(await screen.findByLabelText('Server pairing code')).toHaveValue('psfpt_code');
    expect(codeKinds).toEqual(['peer']);
    expect(screen.getByRole('status')).toHaveTextContent('open Settings, Sync, Remote PSF Guard. Click Add peer');
  });

  it('turns sync for other servers on and off without touching the plugins\' switch', async () => {
    const { saves } = show({ token_configured: true, sync_enabled: true, clients: [plugin, peer] });
    const state = await screen.findByTestId('peer-sync-state');
    expect(state).toHaveAttribute('data-on', 'false');
    fireEvent.click(screen.getByRole('button', { name: 'Turn on sync for other servers' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toMatchObject({ peer_sync_enabled: true, sync_enabled: true });
    await waitFor(() => expect(screen.getByTestId('peer-sync-state')).toHaveTextContent('On. Paired PSF Guard servers merge'));
    fireEvent.click(screen.getByRole('button', { name: 'Turn off sync for other servers' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1]).toMatchObject({ peer_sync_enabled: false, sync_enabled: true });
  });

  it('asks for a server or a key before sync can be turned on', async () => {
    show({ token_configured: true, sync_enabled: true, clients: [plugin] });
    await screen.findByTestId('peer-sync-state');
    expect(screen.getByText('Pair a server or set a remote API key first.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Turn on sync for other servers' })).toBeNull();
  });

  it('reads an older server\'s single switch as the servers\' switch', async () => {
    show({ token_configured: true, key_configured: true, sync_enabled: true, peer_sync_enabled: undefined });
    expect(await screen.findByTestId('peer-sync-state')).toHaveTextContent('On. Paired PSF Guard servers and the remote API key merge');
  });

  it('generates a remote API key, shows it once, and saves it', async () => {
    const { saves } = show({});
    expect(await screen.findByTestId('sync-key-state')).toHaveTextContent('Not set.');
    expect(screen.queryByRole('button', { name: 'Remove key' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Generate key' }));
    const key = screen.getByLabelText('New remote API key');
    expect((key as HTMLInputElement).value).toMatch(/^[0-9a-f]{64}$/);
    expect(screen.getByText('Copy this key now. It is not shown again after you save it.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save key' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].token).toBe((key as HTMLInputElement).value);
    expect(screen.queryByLabelText('New remote API key')).toBeNull();
  });

  it('shows when the key was last used and removes it', async () => {
    const { saves } = show({ token_configured: true, key_configured: true, key_contact: contact });
    expect(await screen.findByText('Last used 5 min ago.')).toBeInTheDocument();
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(true);
    fireEvent.click(screen.getByRole('button', { name: 'Remove key' }));
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('Anything using it stops at once'));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toMatchObject({ clear_token: true });
    expect(saves[0].token).toBeUndefined();
    await waitFor(() => expect(screen.getByTestId('sync-key-state')).toHaveTextContent('Not set.'));
  });

  it('keeps a generated key on screen when saving it fails', async () => {
    show({}, { failSaves: true });
    fireEvent.click(await screen.findByRole('button', { name: 'Generate key' }));
    const key = (screen.getByLabelText('New remote API key') as HTMLInputElement).value;
    fireEvent.click(screen.getByRole('button', { name: 'Save key' }));
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(screen.getByLabelText('New remote API key')).toHaveValue(key);
  });
});
