import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import PlanDeleteDialog from '../director/PlanDeleteDialog';
import type { DirectorPlanDeletion } from '../../api/directorTypes';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const PLAN = '11111111-1111-4111-8111-111111111111';
const path = `/api/director/v1/projects/${PLAN}/deletion`;

function preview(extra: Partial<DirectorPlanDeletion> = {}): DirectorPlanDeletion {
  return {
    name: 'M31',
    databases: [
      { source_project_guid: 'g-c925', catalog_slug: 'c925', catalog_name: 'C925', synced_from: null, project: { id: 3, name: 'M31', state: 2, targets: 2, exposure_plans: 4, frames: 0, digest: 'a' }, problem: null },
      { source_project_guid: 'g-redcat', catalog_slug: 'redcat', catalog_name: 'RedCat', synced_from: null, project: null, problem: null },
    ],
    refusal: null,
    ...extra,
  };
}

function mount(onDeleted = vi.fn(), refetchInterval: number | false = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchInterval } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  render(<PlanDeleteDialog planId={PLAN} name="M31" onClose={vi.fn()} onDeleted={onDeleted} />, { wrapper });
  return onDeleted;
}

describe('Deleting a plan', () => {
  it('lists each database\'s project and deletes them with the plan', async () => {
    const sent: unknown[] = [];
    server.use(
      http.get(path, () => ok(preview())),
      http.post(path, async ({ request }) => {
        sent.push(await request.json());
        return ok({ state: 'deleted', name: 'M31', databases: ['C925'] });
      }),
    );
    const onDeleted = mount();
    expect(await screen.findByText('On C925: the project M31, with 2 targets and 4 exposure plans')).toBeInTheDocument();
    expect(screen.getByText('On RedCat: nothing; the project is no longer there')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Delete plan' }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledWith({ state: 'deleted', name: 'M31', databases: ['C925'] }));
    expect(sent).toEqual([{ digests: { 'g-c925': 'a' } }]);
  });

  it('explains why a plan with frames stays', async () => {
    server.use(http.get(path, () => ok(preview({ refusal: 'C925 has 3 frames of this plan. Delete that project from the Library first.' }))));
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent('C925 has 3 frames of this plan.');
    expect(screen.getByRole('button', { name: 'Delete plan' })).toBeDisabled();
  });

  it('says which projects went when another changed part way', async () => {
    server.use(
      http.get(path, () => ok(preview())),
      http.post(path, () => ok({ state: 'changed', deleted: ['C925'], preview: preview({ databases: [preview().databases[1]] }) })),
    );
    mount();
    await screen.findByText(/On C925: the project M31/);
    fireEvent.click(screen.getByRole('button', { name: 'Delete plan' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('The project on C925 was deleted, but the plan stays. The plan changed after you opened this dialog.');
    expect(screen.queryByText(/On C925: the project M31/)).not.toBeInTheDocument();
  });

  it('keeps its preview while open, whatever the app refreshes', async () => {
    let reads = 0;
    server.use(http.get(path, () => { reads += 1; return ok(preview()); }));
    mount(vi.fn(), 20);
    await screen.findByText(/On C925: the project M31/);
    await new Promise(resolve => setTimeout(resolve, 150));
    expect(reads).toBe(1);
  });

  it('says what went when deleting stopped, and reads the plan again', async () => {
    let reads = 0;
    server.use(
      http.get(path, () => { reads += 1; return ok(preview()); }),
      http.post(path, () => ok({ state: 'stopped', error: 'Could not delete M31 from RedCat: database is locked', deleted: ['C925', 'Askar'] })),
    );
    mount();
    await screen.findByText(/On C925: the project M31/);
    fireEvent.click(screen.getByRole('button', { name: 'Delete plan' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'The projects on C925 and Askar were deleted, but the plan stays. Could not delete M31 from RedCat: database is locked');
    await waitFor(() => expect(reads).toBe(2));
  });

  it('says a synced database keeps the project, and an Active one stops being scheduled', async () => {
    server.use(http.get(path, () => ok(preview({ databases: [
      { ...preview().databases[0], synced_from: '/rig/scheduler.sqlite' },
      { ...preview().databases[0], source_project_guid: 'g-b', catalog_slug: 'b', catalog_name: 'Askar', project: { ...preview().databases[0].project!, state: 1 } },
    ] }))));
    mount();
    expect(await screen.findByText(/C925 copies from/)).toHaveTextContent('The project stays in that database, N.I.N.A. still schedules it there while it is Active, and the next sync adds it back here.');
    expect(screen.getByText('On Askar the project is Active. When N.I.N.A. uses that database, it stops scheduling the project.')).toBeInTheDocument();
  });
});
