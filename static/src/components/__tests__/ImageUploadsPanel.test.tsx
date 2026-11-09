import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import ImageUploadsPanel from '../rigs/ImageUploadsPanel';

const ok = (data: unknown) => ({ success: true, data, error: null });

type Receiver = Record<string, unknown>;
function show(receiver: Receiver, { failSaves = false, expiresIn = 3600 } = {}) {
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
      if (failSaves) return HttpResponse.json({ success: false, data: null, error: 'resolving remote image upload directory' }, { status: 400 });
      current = { ...current, ...Object.fromEntries(Object.entries(body.remote_image_upload).filter(([, value]) => value !== undefined)) };
      return HttpResponse.json(ok({ id: 'remote' }));
    }),
    http.post('/api/databases/remote/pairing-token', () => HttpResponse.json(ok({ pairing_token: 'psfpt_code', expires_at: Math.floor(Date.now() / 1000) + expiresIn }))),
    http.delete('/api/databases/remote/clients/:client', ({ params }) => { revoked.push(String(params.client)); return HttpResponse.json(ok(null)); }),
  );
  const access = { ...({} as ReturnType<typeof useAccess>), canWrite: true };
  render(<AccessContext.Provider value={access}>
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ImageUploadsPanel databaseId="remote" />
    </QueryClientProvider>
  </AccessContext.Provider>);
  return { saves, revoked };
}

describe('Image uploads panel', () => {
  it('turns image uploads on with a receive folder and the database layout, then saves layout changes', async () => {
    // No pairing is needed to turn uploads on: each plugin pairs on its own.
    const { saves } = show({
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

  it('turns image uploads off while no plugin is paired', async () => {
    const { saves } = show({ enabled: true, image_directory: '/images/remote' });
    expect(await screen.findByTestId('sync-upload-state')).toHaveTextContent('On. Paired plugins send image files to /images/remote.');
    fireEvent.click(screen.getByRole('button', { name: 'Turn off image uploads' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toMatchObject({ enabled: false });
  });
});
