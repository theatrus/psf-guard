import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import ProjectDeleteDialog from '../ProjectDeleteDialog';
import type { ProjectRemovalPreview } from '../../api/types';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

function preview(extra: Partial<ProjectRemovalPreview> = {}): ProjectRemovalPreview {
  return {
    project_id: 7, name: 'M31', guid: 'pg', state: 2, targets: 2, exposure_plans: 3,
    frames: { total: 0, accepted: 0, rejected: 0, pending: 0 }, protected_frames: 0,
    digest: 'd1', linked_plan: null, synced_from: null, allowed: true, ...extra,
  };
}

function mount(onDeleted = vi.fn(), refetchInterval: number | false = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchInterval } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  render(<ProjectDeleteDialog request={{ dbId: 'rig', dbName: 'C925', projectId: 7, name: 'M31' }} onClose={vi.fn()} onDeleted={onDeleted} />, { wrapper });
  return onDeleted;
}

const removed = { state: 'removed', name: 'M31', targets: 2, exposure_plans: 3, frames: 0, plan: null, plan_error: null };

describe('Deleting a project', () => {
  it('deletes a project without frames after one click, sending the preview digest', async () => {
    const digests: string[] = [];
    server.use(
      http.get('/api/db/rig/projects/7/removal', () => ok(preview({ linked_plan: { name: 'Andromeda', databases: 1, from_collaboration: false } }))),
      http.post('/api/db/rig/projects/7/removal', async ({ request }) => {
        digests.push((await request.json() as { digest: string }).digest);
        return ok(removed);
      }),
    );
    const onDeleted = mount();
    expect(await screen.findByText('2 targets and 3 exposure plans')).toBeInTheDocument();
    expect(screen.getByText('No frames: nothing was captured for this project.')).toBeInTheDocument();
    expect(screen.getByText(/Its plan, Andromeda, keeps its framing/)).toBeInTheDocument();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Delete project' }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledWith(removed));
    expect(digests).toEqual(['d1']);
  });

  it('asks for the name before deleting frames, and says the files stay', async () => {
    server.use(http.get('/api/db/rig/projects/7/removal', () => ok(preview({
      state: 1, frames: { total: 1234, accepted: 1000, rejected: 200, pending: 34 },
    }))));
    mount();
    expect(await screen.findByText(/1,234 frames \(1,000 accepted, 200 rejected, 34 pending\)/)).toHaveTextContent('The image files stay on disk.');
    expect(screen.getByText(/The project is Active. When N.I.N.A. uses this database/)).toBeInTheDocument();
    const remove = screen.getByRole('button', { name: 'Delete project' });
    expect(remove).toBeDisabled();
    const field = screen.getByRole('textbox', { name: /Type M31 to delete the project and its frames/ });
    fireEvent.change(field, { target: { value: 'M3' } });
    expect(remove).toBeDisabled();
    fireEvent.change(field, { target: { value: 'M31' } });
    expect(remove).toBeEnabled();
  });

  it('keeps its preview while open, whatever the app refreshes', async () => {
    let reads = 0;
    server.use(http.get('/api/db/rig/projects/7/removal', () => {
      reads += 1;
      return ok(preview());
    }));
    mount(vi.fn(), 20);
    await screen.findByText('2 targets and 3 exposure plans');
    await new Promise(resolve => setTimeout(resolve, 150));
    expect(reads).toBe(1);
  });

  it('asks for "delete" when the project has no name', async () => {
    server.use(http.get('/api/db/rig/projects/7/removal', () => ok(preview({
      name: ' ', frames: { total: 2, accepted: 2, rejected: 0, pending: 0 },
    }))));
    mount();
    const field = await screen.findByRole('textbox', { name: /Type delete to delete the project and its frames/ });
    expect(screen.getByRole('button', { name: 'Delete project' })).toBeDisabled();
    fireEvent.change(field, { target: { value: 'delete' } });
    expect(screen.getByRole('button', { name: 'Delete project' })).toBeEnabled();
  });

  it('shows the new counts when the project changed since the preview', async () => {
    const digests: string[] = [];
    server.use(
      http.get('/api/db/rig/projects/7/removal', () => ok(preview())),
      http.post('/api/db/rig/projects/7/removal', async ({ request }) => {
        const { digest } = await request.json() as { digest: string };
        digests.push(digest);
        return digest === 'd1'
          ? ok({ state: 'changed', preview: preview({ digest: 'd2', frames: { total: 1, accepted: 0, rejected: 0, pending: 1 } }) })
          : ok({ ...removed, frames: 1 });
      }),
    );
    const onDeleted = mount();
    await screen.findByText('2 targets and 3 exposure plans');
    fireEvent.click(screen.getByRole('button', { name: 'Delete project' }));
    expect(await screen.findByText(/The project changed after you opened this dialog/)).toBeInTheDocument();
    expect(screen.getByText(/1 frame \(0 accepted/)).toBeInTheDocument();
    // A frame arrived, so the name is asked for now.
    expect(screen.getByRole('button', { name: 'Delete project' })).toBeDisabled();
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'M31' } });
    fireEvent.click(screen.getByRole('button', { name: 'Delete project' }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(digests).toEqual(['d1', 'd2']);
  });

  it.each([
    [{ allowed: false }, /does not allow database changes/],
    [{ linked_plan: { name: 'Shared', databases: 2, from_collaboration: true } }, /belongs to Shared, a plan from a collaboration\. Open the plan and choose Leave collaboration/],
    [{ protected_frames: 2 }, /2 frames of this project were reported to a collaboration/],
  ])('explains why a project cannot be deleted: %o', async (extra, why) => {
    server.use(http.get('/api/db/rig/projects/7/removal', () => ok(preview(extra))));
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent(why);
    expect(screen.getByRole('button', { name: 'Delete project' })).toBeDisabled();
  });

  it('says a synced catalog keeps the project in its source', async () => {
    server.use(http.get('/api/db/rig/projects/7/removal', () => ok(preview({
      state: 1, synced_from: '/rig/schedulerdb.sqlite',
      linked_plan: { name: 'M31', databases: 3, from_collaboration: false },
    }))));
    mount();
    expect(await screen.findByText(/This catalog copies from/)).toHaveTextContent(
      'The project stays in that database, and N.I.N.A. still schedules it there while it is Active.');
    expect(screen.queryByText(/The project is Active. When N.I.N.A. uses this database/)).not.toBeInTheDocument();
    expect(screen.getByText('Its plan, M31, keeps its other 2 databases and no longer plans work for this rig.')).toBeInTheDocument();
  });

  it('says later syncs leave a project without frames out of a synced catalog', async () => {
    server.use(http.get('/api/db/rig/projects/7/removal', () => ok(preview({ synced_from: '/rig/schedulerdb.sqlite' }))));
    mount();
    expect(await screen.findByText(/This catalog copies from/)).toHaveTextContent(
      'Later syncs leave the project out of this catalog.');
  });
});
