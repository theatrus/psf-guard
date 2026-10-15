import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import RescanImageFolders from '../RescanImageFolders';

function mount(dbId: string | null = 'askar') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><RescanImageFolders dbId={dbId} /></QueryClientProvider>);
  return client;
}

describe('RescanImageFolders', () => {
  it('rebuilds the folder tree, then the file index, and refreshes what the database shows', async () => {
    const calls: string[] = [];
    let finishTree!: () => void;
    const treeDone = new Promise<void>((resolve) => { finishTree = resolve; });
    server.use(
      http.put('/api/db/askar/refresh-directory-cache', async () => {
        calls.push('tree');
        await treeDone;
        return HttpResponse.json({ success: true, data: { tree: null }, error: null });
      }),
      http.put('/api/db/askar/refresh-cache', () => {
        calls.push('files');
        return HttpResponse.json({ success: true, data: { checked: 0 }, error: null });
      }),
    );
    const client = mount();
    const stale = client.getQueryCache().build(client, { queryKey: ['db', 'askar', 'images'], queryFn: () => [] });
    await userEvent.click(screen.getByRole('button', { name: 'Rescan image folders' }));
    // Held while it works, and says so.
    expect(await screen.findByRole('button', { name: 'Rescanning image folders…' })).toBeDisabled();
    finishTree();
    await waitFor(() => expect(calls).toEqual(['tree', 'files']));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Rescan image folders' })).toBeEnabled());
    expect(stale.state.isInvalidated).toBe(true);
  });

  it('waits for a database', () => {
    mount(null);
    expect(screen.getByRole('button', { name: 'Rescan image folders' })).toBeDisabled();
  });
});
