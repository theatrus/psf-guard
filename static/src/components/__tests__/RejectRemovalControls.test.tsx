import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import RejectRemovalControls, { PurgeRejectsButton } from '../RejectRemovalControls';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const NOW = Math.floor(Date.now() / 1000);

function mount(canManage = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return render(<div className="tauri-settings"><RejectRemovalControls dbId="rig" canManage={canManage} /></div>, { wrapper });
}

/** The catalog's reject removal job, mocked: a run finishes as it starts,
 *  unless `hold` keeps it running for the page to follow and stop. */
function mockJob(handlers: {
  preview?: (body: unknown) => object;
  apply?: (body: unknown) => object;
  restore?: (body: unknown) => object;
  empty?: () => object;
}) {
  let status: Record<string, unknown> = {
    state: 'idle', action: null, stage: null, done: 0, total: 0, run: 0, started_at: null,
    finished_at: null, plan: null, changed: false, outcome: null, error: null, priority: null,
  };
  const finish = (action: string, change: (run: number) => object) => {
    const run = (status.run as number) + 1;
    status = { ...status, run, action, state: 'done', stage: null, changed: false, finished_at: run, ...change(run) };
    return HttpResponse.json({ success: true, data: status, error: null }, { status: 202 });
  };
  server.use(
    http.get('/api/db/rig/rejects/removal', () => ok(status)),
    http.post('/api/db/rig/rejects/removal/preview', async ({ request }) => {
      const body = await request.json();
      return finish('preview', () => ({ plan: handlers.preview?.(body) }));
    }),
    http.post('/api/db/rig/rejects/removal/apply', async ({ request }) => {
      const body = await request.json();
      return finish('remove', run => handlers.apply?.(body) ?? {
        ...status,
        plan: null,
        outcome: { run, finished_at: run, action: 'remove', removed: { batch_id: 'b1', removed: [], failed: [], files_moved: 0, bytes: 0, trash_until: 0 } },
      });
    }),
    http.post('/api/db/rig/rejects/removed/restore', async ({ request }) => {
      const body = await request.json();
      return finish('restore', run => ({ outcome: { run, finished_at: run, action: 'restore', restored: handlers.restore?.(body) } }));
    }),
    http.post('/api/db/rig/rejects/trash/empty', () =>
      finish('empty_trash', run => ({ outcome: { run, finished_at: run, action: 'empty_trash', emptied: handlers.empty?.() } }))),
  );
}

describe('Remove rejects', () => {
  it('previews with the chosen age, removes with the digest, and lists the batch to restore', async () => {
    const previews: unknown[] = [];
    const applies: unknown[] = [];
    let removed = false;
    mockJob({
      preview: body => {
        previews.push(body);
        return { min_age_days: 3, waiting: 2, next_eligible_at: NOW + 86_400, without_files: 0, bytes: 3 * 1024 ** 2, digest: 'd'.repeat(64),
          frames: [{ image_id: 1, guid: 'g1', target_id: 1, target_name: 'M42', file_name: 'M42_Ha_001.fits', rejected_at: NOW - 9 * 86_400, reject_reason: 'Clouds', files: [] }],
          skipped: [{ image_id: 5, guid: 'g5', target_name: 'M42', file_name: null, reason: 'it is in a collaboration report' }] };
      },
      apply: body => {
        applies.push(body);
        removed = true;
        return { plan: null, outcome: { run: 2, finished_at: 2, action: 'remove',
          removed: { batch_id: 'b1', removed: [{ image_id: 1, guid: 'g1' }], failed: [], files_moved: 1, bytes: 3 * 1024 ** 2, trash_until: NOW + 14 * 86_400 } } };
      },
      restore: () => ({ restored: [{ guid: 'g1', image_id: 1 }], failed: [], renamed: [] }),
    });
    server.use(
      http.get('/api/db/rig/rejects/removed', () => ok({ frames: [], batches: removed
        ? [{ batch_id: 'b1', removed_at: NOW, frames: 1, bytes: 3 * 1024 ** 2, trash_until: NOW + 14 * 86_400, files_deleted: 0, purged: 0 }] : [] })),
    );
    mount();
    const group = screen.getByRole('group', { name: 'Remove rejects' });
    fireEvent.change(within(group).getByLabelText('Days rejected'), { target: { value: '3' } });
    fireEvent.click(within(group).getByRole('button', { name: 'Preview' }));
    const plan = await screen.findByTestId('reject-removal-plan');
    expect(previews).toEqual([{ min_age_days: 3 }]);
    expect(plan).toHaveTextContent('1 reject to remove (3.0 MB).');
    expect(plan).toHaveTextContent('2 rejects rejected more recently');
    expect(plan).toHaveTextContent('1 stay: it is in a collaboration report.');
    fireEvent.click(within(group).getByRole('button', { name: 'Remove 1 reject' }));
    expect(await within(group).findByRole('status')).toHaveTextContent('Removed 1 reject; files remain in the trash until');
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
    mockJob({ empty: () => { emptied = true; return { frames: 2, files_deleted: 4, bytes: 1024 ** 2, problems: [] }; } });
    server.use(
      http.get('/api/db/rig/rejects/removed', () => ok({ frames: [], batches: [
        { batch_id: 'old', removed_at: NOW - 30 * 86_400, frames: 2, bytes: 1024 ** 2, trash_until: NOW - 86_400,
          files_deleted: emptied ? 2 : 0, purged: purged ? 2 : 0 },
      ] })),
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

  it('shows the new preview when the rejects changed, and offers nothing without database management', async () => {
    const stale = { min_age_days: 7, waiting: 0, next_eligible_at: null, without_files: 0, bytes: 1, digest: 'a'.repeat(64),
      frames: [{ image_id: 1, guid: 'g1', target_id: 1, target_name: 'M42', file_name: 'x.fits', rejected_at: 0, reject_reason: null, files: [] }], skipped: [] };
    mockJob({
      preview: () => stale,
      // The rejects changed: nothing removed, the preview as they stand now.
      apply: () => ({ changed: true, outcome: null, plan: { ...stale, digest: 'b'.repeat(64), frames: [] } }),
    });
    const { unmount } = mount();
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Remove 1 reject' }));
    expect(await screen.findByRole('status')).toHaveTextContent('The rejected frames changed since the preview. Review the updated preview.');
    expect(screen.getByTestId('reject-removal-plan')).toHaveTextContent('Nothing to remove.');
    unmount();
    mount(false);
    expect(screen.queryByRole('group', { name: 'Remove rejects' })).not.toBeInTheDocument();
  });

  it('follows a removal under way, and stops it between frames', async () => {
    let status: Record<string, unknown> = {
      state: 'running', action: 'remove', stage: 'moving', done: 37, total: 203, run: 4, started_at: 1,
      finished_at: null, plan: null, changed: false, outcome: null, error: null, priority: 'interactive',
    };
    const stops: string[] = [];
    server.use(
      http.get('/api/db/rig/rejects/removal', () => ok(status)),
      http.get('/api/db/rig/rejects/removed', () => ok({ frames: [], batches: [] })),
      http.post('/api/db/rig/rejects/removal/stop', () => {
        stops.push('stop');
        status = { ...status, state: 'stopped', stage: null, finished_at: 9,
          outcome: { run: 4, finished_at: 9, action: 'remove', removed: { batch_id: 'b4', removed: [], failed: [], files_moved: 0, bytes: 0, trash_until: 0, stopped: true } } };
        return ok(status);
      }),
    );
    mount();
    const progress = await screen.findByRole('status', { name: 'Reject removal progress' });
    expect(progress).toHaveTextContent('Removing rejects · Moving to the trash 37 of 203');
    // A run under way holds every other action.
    expect(screen.getByRole('button', { name: 'Preview' })).toBeDisabled();
    fireEvent.click(within(progress).getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(stops).toEqual(['stop']));
    await waitFor(() => expect(screen.queryByRole('status', { name: 'Reject removal progress' })).not.toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Preview' })).toBeEnabled();
  });

  it('opens the removal flow in a Purge Rejects dialog, only with database management', async () => {
    server.use(http.get('/api/db/rig/rejects/removed', () => HttpResponse.json({ success: true, error: null, data: { batches: [] } })));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const wrap = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
    const { unmount } = render(<PurgeRejectsButton dbId="rig" dbName="C925" canManage={false} />, { wrapper: wrap });
    expect(screen.queryByRole('button', { name: 'Purge rejects in C925' })).toBeNull();
    unmount();

    render(<PurgeRejectsButton dbId="rig" dbName="C925" canManage />, { wrapper: wrap });
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Purge rejects in C925' }));
    const dialog = await screen.findByRole('dialog', { name: 'Purge rejects — C925' });
    expect(within(dialog).getByRole('group', { name: 'Remove rejects' })).toBeInTheDocument();
    expect(within(dialog).getByLabelText('Days rejected')).toHaveValue(7);
  });
});
