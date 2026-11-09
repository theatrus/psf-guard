import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import SyncPluginPanel from '../rigs/SyncPluginPanel';

const ok = (data: unknown) => ({ success: true, data, error: null });
const at = Math.floor(Date.now() / 1000) - 5 * 60;
const paired = { client_uuid: 'a', name: 'Observatory PC', paired_at: at, last_contact: { at, version: '1.4.0.0', capabilities: ['waiting_profile_v1'], places_new_projects: true } };

type Receiver = Record<string, unknown>;
function show(receiver: Receiver) {
  let current: Receiver = {
    enabled: false, sync_enabled: false, token_configured: false, key_configured: false, clients: [], placement: 'flat',
    directory_template: '%TARGET%/%DATE%/%TYPE%', directory_template_source: 'preset', directory_template_samples: 0, ...receiver,
  };
  const saves: Record<string, unknown>[] = [];
  const revoked: string[] = [];
  server.use(
    http.get('/api/databases', () => HttpResponse.json(ok([{ id: 'remote', name: 'Remote', database_path: '/remote.sqlite', image_directories: ['/images/remote', '/images/other'], remote_image_upload: current }]))),
    http.put('/api/databases/remote', async ({ request }) => {
      const body = await request.json() as { remote_image_upload: Record<string, unknown> };
      saves.push(body.remote_image_upload);
      current = { ...current, ...Object.fromEntries(Object.entries(body.remote_image_upload).filter(([, value]) => value !== undefined)) };
      return HttpResponse.json(ok({ id: 'remote' }));
    }),
    http.post('/api/databases/remote/pairing-token', () => HttpResponse.json(ok({ pairing_token: 'psfpt_code', expires_at: Math.floor(Date.now() / 1000) + 3600 }))),
    http.delete('/api/databases/remote/clients/:client', ({ params }) => { revoked.push(String(params.client)); return HttpResponse.json(ok(null)); }),
  );
  const access = { ...({} as ReturnType<typeof useAccess>), canWrite: true };
  render(<AccessContext.Provider value={access}>
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <SyncPluginPanel databaseId="remote" />
    </QueryClientProvider>
  </AccessContext.Provider>);
  return { saves, revoked };
}

describe('Sync plugin panel', () => {
  it('lists paired plugins with their last contact and points out an older one', async () => {
    const { revoked } = show({
      token_configured: true,
      clients: [paired, { client_uuid: 'b', name: 'Old laptop', paired_at: at, last_contact: { at, capabilities: [], places_new_projects: false } }, { client_uuid: 'c', name: 'Spare', paired_at: at }],
    });
    const list = await screen.findByRole('list', { name: 'Paired Sync plugins' });
    expect(within(list).getByText(/Observatory PC/).closest('li')).toHaveTextContent('last contact 5 min ago, plugin 1.4.0.0');
    expect(within(list).getByText(/Spare/).closest('li')).toHaveTextContent('no contact recorded');
    expect(screen.getByRole('note')).toHaveTextContent('Old laptop is running an older Sync plugin.');
    vi.spyOn(window, 'confirm').mockReturnValueOnce(true);
    fireEvent.click(screen.getByRole('button', { name: 'Remove Spare' }));
    await waitFor(() => expect(revoked).toEqual(['c']));
  });

  it('generates a pairing code with the steps for N.I.N.A.', async () => {
    show({});
    expect(await screen.findByText(/Install PSF Guard Sync from the N.I.N.A. plugin repository/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Generate Sync pairing code' }));
    expect(await screen.findByLabelText('Sync pairing code')).toHaveValue('psfpt_code');
    expect(screen.getByRole('status')).toHaveTextContent('Pairing turns on scheduler sync.');
  });

  it('turns scheduler sync on and off at once, keeping the other settings', async () => {
    const { saves } = show({ token_configured: true, clients: [paired], enabled: true, image_directory: '/images/remote' });
    const state = await screen.findByTestId('sync-scheduler-state');
    expect(state).toHaveAttribute('data-on', 'false');
    fireEvent.click(screen.getByRole('button', { name: 'Turn on scheduler sync' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toEqual({ enabled: true, image_directory: '/images/remote', sync_enabled: true, placement: 'flat' });
    await waitFor(() => expect(screen.getByTestId('sync-scheduler-state')).toHaveAttribute('data-on', 'true'));
    fireEvent.click(screen.getByRole('button', { name: 'Turn off scheduler sync' }));
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1]).toMatchObject({ sync_enabled: false, enabled: true });
  });

  it('asks for pairing before sync or uploads can be turned on', async () => {
    show({});
    await screen.findByTestId('sync-scheduler-state');
    expect(screen.getAllByText('Pair the plugin first.')).toHaveLength(2);
    expect(screen.queryByRole('button', { name: 'Turn on scheduler sync' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Turn on image uploads' })).toBeNull();
  });

  it('turns image uploads on with a receive folder and the database layout, then saves layout changes', async () => {
    const { saves } = show({
      token_configured: true, clients: [paired],
      catalog_directory_template: '2026/%TARGET%/%NIGHT%/%TYPE%', directory_template_source: 'catalog', directory_template_samples: 18,
    });
    expect(await screen.findByTestId('sync-upload-state')).toHaveAttribute('data-on', 'false');
    const turnOn = screen.getByRole('button', { name: 'Turn on image uploads' });
    expect(turnOn).toBeDisabled();
    fireEvent.change(screen.getByLabelText('Receive folder'), { target: { value: '/images/remote' } });
    fireEvent.change(screen.getByLabelText('Folder layout'), { target: { value: 'target_tree' } });
    const layout = screen.getByLabelText('Layout');
    expect(layout).toHaveValue('catalog');
    expect(within(layout).getAllByRole('option').map(option => option.textContent)).toEqual([
      'Match database: 2026/%TARGET%/%NIGHT%/%TYPE%',
      '%YEAR%/%TARGET%/%NIGHT%/%TYPE%', '%TARGET%/%NIGHT%/%TYPE%', '%TARGET%/%DATE%/%TYPE%', '%TARGET%/%TYPE%/%FILTER%', '%NIGHT%/%TARGET%/%TYPE%', 'Custom',
    ]);
    expect(screen.getByText('Detected from 18 database images:')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Turn on image uploads' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    // The database's match stays: no layout is sent with it.
    expect(saves[0]).toMatchObject({ enabled: true, image_directory: '/images/remote', placement: 'target_tree', directory_template_source: 'catalog' });
    expect(saves[0].directory_template).toBeUndefined();

    await waitFor(() => expect(screen.getByTestId('sync-upload-state')).toHaveTextContent('On. Paired plugins send image files to /images/remote.'));
    // A rescan asked for, then a layout chosen: the choice cancels it.
    const rescan = screen.getByRole('button', { name: 'Rescan' });
    fireEvent.click(rescan);
    expect(screen.getByRole('button', { name: 'Rescan on save' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.change(screen.getByLabelText('Layout'), { target: { value: 'custom' } });
    expect(screen.getByRole('button', { name: 'Rescan' })).toHaveAttribute('aria-pressed', 'false');
    const save = screen.getByRole('button', { name: 'Save upload settings' });
    expect(save).toBeDisabled();
    expect(screen.getByText("Enter the custom layout's folders.")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Custom layout'), { target: { value: '%TARGET%/%DATE%/%TYPE%/%FILTER%' } });
    fireEvent.click(save);
    await waitFor(() => expect(saves).toHaveLength(2));
    expect(saves[1]).toMatchObject({ placement: 'target_tree', directory_template: '%TARGET%/%DATE%/%TYPE%/%FILTER%', directory_template_source: 'preset' });
    expect(saves[1].rescan_directory_layout).toBeUndefined();

    // Buttons wait while a save is under way.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Turn off image uploads' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'Turn off image uploads' }));
    await waitFor(() => expect(saves).toHaveLength(3));
    expect(saves[2]).toMatchObject({ enabled: false });
  });

  it('generates a remote API key, shows it once, and saves it', async () => {
    const { saves } = show({});
    expect(await screen.findByTestId('sync-key-state')).toHaveTextContent('Not set.');
    fireEvent.click(screen.getByRole('button', { name: 'Generate key' }));
    const key = screen.getByLabelText('New remote API key');
    expect((key as HTMLInputElement).value).toMatch(/^[0-9a-f]{64}$/);
    expect(screen.getByText('Copy this key now. It is not shown again after you save it.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save key' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].token).toBe((key as HTMLInputElement).value);
    expect(screen.queryByLabelText('New remote API key')).toBeNull();
  });
});
