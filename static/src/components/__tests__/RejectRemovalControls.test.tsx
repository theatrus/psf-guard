import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import RejectRemovalControls from '../RejectRemovalControls';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const NOW = Math.floor(Date.now() / 1000);

function mount(canManage = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return render(<div className="tauri-settings"><RejectRemovalControls dbId="rig" canManage={canManage} /></div>, { wrapper });
}

describe('Remove rejects', () => {
  it('previews with the chosen age, removes with the digest, and lists the batch to restore', async () => {
    const previews: unknown[] = [];
    const applies: unknown[] = [];
    let removed = false;
    server.use(
      http.post('/api/db/rig/rejects/removal/preview', async ({ request }) => {
        previews.push(await request.json());
        return ok({ min_age_days: 3, waiting: 2, next_eligible_at: NOW + 86_400, without_files: 0, bytes: 3 * 1024 ** 2, digest: 'd'.repeat(64),
          frames: [{ image_id: 1, guid: 'g1', target_id: 1, target_name: 'M42', file_name: 'M42_Ha_001.fits', rejected_at: NOW - 9 * 86_400, reject_reason: 'Clouds', files: [] }],
          skipped: [{ image_id: 5, guid: 'g5', target_name: 'M42', file_name: null, reason: 'a collaboration report names it' }] });
      }),
      http.post('/api/db/rig/rejects/removal/apply', async ({ request }) => {
        applies.push(await request.json());
        removed = true;
        return ok({ batch_id: 'b1', removed: [{ image_id: 1, guid: 'g1' }], failed: [], files_moved: 1, bytes: 3 * 1024 ** 2, trash_until: NOW + 14 * 86_400 });
      }),
      http.get('/api/db/rig/rejects/removed', () => ok({ frames: [], batches: removed
        ? [{ batch_id: 'b1', removed_at: NOW, frames: 1, bytes: 3 * 1024 ** 2, trash_until: NOW + 14 * 86_400, files_deleted: 0, purged: 0 }] : [] })),
      http.post('/api/db/rig/rejects/removed/restore', () => ok({ restored: [{ guid: 'g1', image_id: 1 }], failed: [], renamed: [] })),
    );
    mount();
    const group = screen.getByRole('group', { name: 'Remove rejects' });
    fireEvent.change(within(group).getByLabelText('Days rejected'), { target: { value: '3' } });
    fireEvent.click(within(group).getByRole('button', { name: 'Preview' }));
    const plan = await screen.findByTestId('reject-removal-plan');
    expect(previews).toEqual([{ min_age_days: 3 }]);
    expect(plan).toHaveTextContent('1 reject to remove (3.0 MB).');
    expect(plan).toHaveTextContent('2 rejects rejected more recently');
    expect(plan).toHaveTextContent('1 stay: a collaboration report names it.');
    fireEvent.click(within(group).getByRole('button', { name: 'Remove 1 reject' }));
    expect(await within(group).findByRole('status')).toHaveTextContent('Removed 1 reject; their files wait in the trash until');
    expect(applies).toEqual([{ min_age_days: 3, digest: 'd'.repeat(64), retention_days: 14 }]);
    const batches = await screen.findByRole('list', { name: 'Removed rejects' });
    expect(batches).toHaveTextContent('1 reject · 3.0 MB · in the trash until');
    fireEvent.click(within(batches).getByRole('button', { name: /^Restore the rejects removed/ }));
    await waitFor(() => expect(within(group).getByRole('status')).toHaveTextContent('Restored 1 reject.'));
    // Nothing is past its retention yet, so the trash cannot be emptied.
    expect(within(group).queryByRole('button', { name: 'Empty trash' })).not.toBeInTheDocument();
  });

  it('empties the trash past its retention, then purges that batch', async () => {
    let emptied = false;
    let purged = false;
    server.use(
      http.get('/api/db/rig/rejects/removed', () => ok({ frames: [], batches: [
        { batch_id: 'old', removed_at: NOW - 30 * 86_400, frames: 2, bytes: 1024 ** 2, trash_until: NOW - 86_400,
          files_deleted: emptied ? 2 : 0, purged: purged ? 2 : 0 },
      ] })),
      http.post('/api/db/rig/rejects/trash/empty', () => { emptied = true; return ok({ frames: 2, files_deleted: 4, bytes: 1024 ** 2, problems: [] }); }),
      http.post('/api/db/rig/rejects/removed/purge', async ({ request }) => {
        expect(await request.json()).toEqual({ batch_id: 'old' });
        purged = true;
        return ok({ frames: 2, bytes: 5000 });
      }),
    );
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Empty trash' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Deleted 4 files (1.0 MB) from the trash.');
    const batches = screen.getByRole('list', { name: 'Removed rejects' });
    await waitFor(() => expect(batches).toHaveTextContent('files deleted'));
    expect(within(batches).queryByRole('button', { name: /^Restore/ })).not.toBeInTheDocument();
    fireEvent.click(within(batches).getByRole('button', { name: /^Purge the rejects removed/ }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Purged the saved records of 2 rejects; they can no longer be restored.'));
    await waitFor(() => expect(batches).toHaveTextContent('purged'));
    expect(within(batches).queryByRole('button', { name: /^Purge/ })).not.toBeInTheDocument();
  });

  it('says a stale preview changed and offers nothing without database management', async () => {
    server.use(
      http.post('/api/db/rig/rejects/removal/preview', () => ok({ min_age_days: 7, waiting: 0, next_eligible_at: null, without_files: 0, bytes: 1, digest: 'a'.repeat(64),
        frames: [{ image_id: 1, guid: 'g1', target_id: 1, target_name: 'M42', file_name: 'x.fits', rejected_at: 0, reject_reason: null, files: [] }], skipped: [] })),
      http.post('/api/db/rig/rejects/removal/apply', () => HttpResponse.json({ success: false, data: null, error: 'stale' }, { status: 409 })),
    );
    const { unmount } = mount();
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Remove 1 reject' }));
    expect(await screen.findByRole('status')).toHaveTextContent('The rejects changed since the preview; preview again.');
    expect(screen.queryByTestId('reject-removal-plan')).not.toBeInTheDocument();
    unmount();
    mount(false);
    expect(screen.queryByRole('group', { name: 'Remove rejects' })).not.toBeInTheDocument();
  });
});
