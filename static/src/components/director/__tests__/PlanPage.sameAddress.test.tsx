import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../../test/msw-server';
import { AccessContext, useAccess } from '../../../auth/access';
import type { DirectorPlanLink, DirectorPlanRow } from '../../../api/directorTypes';
import PlanPage from '../PlanPage';

const mounts = vi.hoisted(() => [] as string[]);

// The workspace holds its edits in component state, as the real one does: a
// remount is what lost them. This one keeps one field and the real save bar.
vi.mock('../ProjectWorkspace', async () => {
  const { useEffect, useState } = await vi.importActual<typeof import('react')>('react');
  const state = await vi.importActual<typeof import('../pageDraftsState')>('../pageDraftsState');
  const bar = await vi.importActual<typeof import('../pageDrafts')>('../pageDrafts');
  function Goal() {
    const [goal, setGoal] = useState('40');
    state.useDraftSection('plan', { label: 'Exposures', order: 2, unsaved: goal !== '40', save: async () => true, discard: () => setGoal('40') });
    return <input aria-label="Goal" value={goal} onChange={event => setGoal(event.target.value)} />;
  }
  function Workspace({ projectId }: { projectId: string }) {
    const drafts = state.usePageDrafts();
    useEffect(() => { mounts.push(projectId); }, [projectId]);
    return <bar.DraftProvider drafts={drafts}><bar.SaveBar drafts={drafts} canWrite pageKeys={['plan']} /><Goal /></bar.DraftProvider>;
  }
  return { default: Workspace };
});

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const record = { id: '11111111-1111-4111-8111-111111111111', name: 'M31', revision: 1 };
const other = { id: '66666666-6666-4666-8666-666666666666', name: 'M31 on RC51', revision: 1 };
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
const link = (slug: string, row: number): DirectorPlanLink => ({ catalog_slug: slug, catalog_name: slug.toUpperCase(), rig, source_project_guid: 'abcd-1', source_row_id: row, source_name: 'M31', source_state: 1, earliest_capture_s: null, latest_capture_s: null, targets: [] });
const row = (project: typeof record, links: DirectorPlanLink[]): DirectorPlanRow => ({ project, links, progress: null, framing: null, plan: null, activation: null });

describe('plan page address', () => {
  it('keeps the open workspace and its edits when the plan takes a new key', async () => {
    mounts.length = 0;
    let rows = [row(record, [link('c925', 7)])];
    server.use(
      http.get('/api/director/v1/status', () => ok({ protocol_version: 1, enabled: true, instance_id: 'instance', database_management: true })),
      http.get('/api/director/v1/plans', () => ok({ rows, warnings: [] })),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    function Wrapper({ children }: { children: ReactNode }) {
      const access = useAccess();
      return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite: true }}>{children}</AccessContext.Provider></QueryClientProvider>;
    }
    const router = createMemoryRouter([
      { path: '/plan', element: <PlanPage /> },
      { path: '/', element: <p>The Library</p> },
    ], { initialEntries: ['/plan?plan=abcd-1'] });
    render(<RouterProvider router={router} />, { wrapper: Wrapper });
    fireEvent.change(await screen.findByLabelText('Goal'), { target: { value: '120' } });

    // A detach leaves a second plan holding the GUID, so this plan's key
    // becomes its id. The address follows; the workspace stays as it was.
    rows = [row(record, [link('c925', 7)]), row(other, [link('rc51', 3)])];
    await act(() => client.invalidateQueries({ queryKey: ['directorPlans'] }));
    await waitFor(() => expect(router.state.location.search).toBe(`?plan=${record.id}`));
    expect(screen.getByLabelText('Goal')).toHaveValue('120');
    expect(mounts).toEqual([record.id]);
    expect(screen.queryByRole('alert')).toBeNull();

    // Opening the other plan is leaving this one: the save bar asks.
    await act(() => router.navigate(`/plan?plan=${other.id}`));
    expect(await screen.findByRole('alert')).toHaveTextContent('Leave with unsaved changes in Exposures?');
    expect(router.state.location.search).toBe(`?plan=${record.id}`);
  });
});
