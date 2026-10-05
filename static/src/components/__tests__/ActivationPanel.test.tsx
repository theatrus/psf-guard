import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import ActivationPanel from '../director/ActivationPanel';
import { DraftProvider } from '../director/pageDrafts';
import { useDraftSection, usePageDrafts } from '../director/pageDraftsState';
import type { DirectorActivationReport } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const rig = { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 };
function report(applied: boolean): DirectorActivationReport {
  return {
    project: { id: 'project', name: 'Heart', revision: 1 }, framing_revision: 2, plan_revision: 3, panels: 2,
    rigs: [{ rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile_id: 'p', applied, warnings: [], push: null, changes: [
      { kind: 'project', action: 'create', name: 'Heart', detail: 'new Target Scheduler project' },
      { kind: 'target', action: 'create', name: 'Heart r1c1', detail: '02h 32m, +61° 27′' },
      { kind: 'target', action: 'create', name: 'Heart r2c1', detail: '02h 32m, +60° 03′' },
      { kind: 'plan', action: 'create', name: 'Heart r1c1 · Ha 300', detail: '72 frames of 300 s' },
      { kind: 'plan', action: 'create', name: 'Heart r2c1 · Ha 300', detail: '72 frames of 300 s' },
    ] }, { rig: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'Remote', revision: 1 }, catalog_slug: null, catalog_name: 'Remote', profile_id: null, applied: false, changes: [], push: null, warnings: ['Rig has no registered database on this server; push it through Sync later.'] },
    { rig: { id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Desert', revision: 1 }, catalog_slug: 'desert', catalog_name: 'Desert copy', profile_id: 'p', applied, changes: [{ kind: 'project', action: 'create', name: 'Heart', detail: 'new Target Scheduler project' }], warnings: [],
      push: { peer_id: 'obs', peer_name: 'Observatory', applied, summary: applied ? { project_inserted: 1 } : {}, error: null } }],
    warnings: [], preview_digest: 'd'.repeat(64), applied, activation_revision: applied ? 1 : null,
  };
}
function fixture(conflict = false, activated = false) {
  const applies: Array<{ preview_digest: string }> = [];
  let previews = 0;
  let pushes = 0;
  server.use(
    http.get('/api/director/v1/projects/project/activation', () => HttpResponse.json(ok({ activation: activated
      ? { project_id: 'project', revision: 2, framing_revision: 2, plan_revision: 3, coordinator_instance_id: 'c', applied_at_ms: 1_700_000_000_000, rigs: [] } : null }))),
    http.post('/api/director/v1/projects/project/activation/push', () => { pushes++; return HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, activation_revision: 2, warnings: [],
      rigs: [{ rig: { id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Desert', revision: 1 }, catalog_name: 'Desert copy', push: { peer_id: 'obs', peer_name: 'Observatory', applied: false, summary: {}, error: 'peer refused the key' } }] })); }),
    http.post('/api/director/v1/projects/project/activation/preview', () => { previews++; return HttpResponse.json(ok(report(false))); }),
    http.post('/api/director/v1/projects/project/activation/apply', async ({ request }) => {
      applies.push(await request.json() as { preview_digest: string });
      if (conflict) return HttpResponse.json({ success: false, data: null, error: 'Director record conflicts with stored content; reload before retrying' }, { status: 409 });
      return HttpResponse.json(ok(report(true)));
    }),
  );
  return { applies, previewCount: () => previews, pushCount: () => pushes };
}
/** A page with one draft section, as the project workspace has. */
function Page({ unsaved, save }: { unsaved: boolean; save: () => Promise<boolean> }) {
  const drafts = usePageDrafts();
  return <DraftProvider drafts={drafts}><Section unsaved={unsaved} save={save} /><ActivationPanel projectId="project" /></DraftProvider>;
}
function Section({ unsaved, save }: { unsaved: boolean; save: () => Promise<boolean> }) {
  useDraftSection('plan', { label: 'Plan', order: 2, unsaved, save, discard: () => {} });
  return null;
}
function mount(canWrite = true, plan: { unsavedPlan?: boolean; savePlan?: () => Promise<boolean> } = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  const node = plan.savePlan ? <Page unsaved={!!plan.unsavedPlan} save={plan.savePlan} /> : <ActivationPanel projectId="project" />;
  return render(node, { wrapper: Wrapper });
}

describe('Activation panel', () => {
  it('previews per rig, applies with the preview digest, and reports what landed', async () => {
    const { applies } = fixture(); mount();
    expect(screen.queryByRole('button', { name: /Apply to rig databases/ })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole('button', { name: 'Preview activation' }));
    // Rigs no block shows elsewhere are listed here, each with its counts.
    const redcat = await screen.findByLabelText('RedCat 61 activation');
    expect(redcat).toHaveTextContent('Preview · Project: 1 new · Targets: 2 new · Exposure plans: 2 new');
    expect(screen.getByText(/push it through Sync later/)).toBeInTheDocument();
    expect(screen.getByLabelText('Desert copy activation')).toHaveTextContent('Will push to Observatory');
    fireEvent.click(screen.getByRole('button', { name: 'Apply to rig databases' }));
    expect(await screen.findByText('Pushed to Observatory', { exact: false })).toBeInTheDocument();
    expect(screen.getByLabelText('RedCat 61 activation')).toHaveTextContent('Applied');
    expect(applies).toEqual([{ preview_digest: 'd'.repeat(64) }]);
    expect(screen.getByText(/Activation revision 1/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Apply to rig databases' })).not.toBeInTheDocument();
  });

  it('says which existing rows are taken over and which plans are left as they are', async () => {
    fixture(); mount();
    const takeover: DirectorActivationReport = { ...report(false), rigs: [{ rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile_id: 'p', applied: false, warnings: [], push: null, changes: [
      { kind: 'project', action: 'update', name: 'Heart', detail: 'linked Target Scheduler project' },
      { kind: 'target', action: 'adopt', name: 'IC 1805 r1c1', detail: 'takes over the existing target at 02h 32m +61° 27′' },
      { kind: 'template', action: 'create', name: '#4 OIII 300', detail: 'OIII: no template in this database has these settings' },
      { kind: 'plan', action: 'adopt', name: 'IC 1805 r1c1 · Ha 300 · 300 s', detail: 'takes over plan #11 (12 of 40 frames taken); desired 40 → 72' },
      { kind: 'plan', action: 'create', name: 'IC 1805 r1c1 · OIII 300 · 300 s', detail: '72 frames, template #4 OIII 300' },
      { kind: 'plan', action: 'keep', name: 'IC 1805 r1c1 · Ha 300 · 600 s', detail: 'plan #12 (0 of 10 frames taken) is not part of this plan; left as it is' },
    ] }] };
    server.use(http.post('/api/director/v1/projects/project/activation/preview', () => HttpResponse.json(ok(takeover))));
    fireEvent.click(await screen.findByRole('button', { name: 'Preview activation' }));
    const rigSummary = await screen.findByLabelText('RedCat 61 activation');
    expect(rigSummary).toHaveTextContent('Targets: 1 taken over');
    expect(rigSummary).toHaveTextContent('Exposure plans: 1 new, 1 taken over, 1 left as is');
    const changes = screen.getByText('What changes in RedCat 61').closest('details')!;
    expect(changes).toHaveAttribute('open');
    expect(changes).toHaveTextContent('Taken overIC 1805 r1c1 · Ha 300 · 300 s takes over plan #11 (12 of 40 frames taken); desired 40 → 72');
    expect(changes).toHaveTextContent('Left as isIC 1805 r1c1 · Ha 300 · 600 s plan #12');
    expect(changes).toHaveTextContent('Exposure templates');
    expect(changes).toHaveTextContent('New#4 OIII 300');
  });

  it('drops a stale preview when apply is refused', async () => {
    const { previewCount } = fixture(true); mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Preview activation' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Apply to rig databases' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Preview again before applying');
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Apply to rig databases' })).not.toBeInTheDocument());
    expect(previewCount()).toBe(1);
  });

  it('pushes the last activation again and shows a peer that refused', async () => {
    const { pushCount } = fixture(false, true); mount();
    expect(await screen.findByText(/Last activated revision 2/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Push to remote sites again' }));
    expect(await screen.findByText(/Pushed again/)).toBeInTheDocument();
    expect(screen.getByText('Desert copy: Push to Observatory failed: peer refused the key')).toBeInTheDocument();
    expect(pushCount()).toBe(1);
  });

  it('offers nothing to a read-only account', async () => {
    fixture(); mount(false);
    expect(await screen.findByText('Read only')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Preview activation' })).not.toBeInTheDocument();
  });
  it('saves unsaved plan edits before previewing, and hides Apply until it does', async () => {
    // Activation writes the saved plan: a goal raised in the editor but not
    // saved would otherwise be applied as it was before, and look lost.
    const { previewCount } = fixture();
    const order: string[] = [];
    const savePlan = vi.fn(async () => { order.push(`save at ${previewCount()} previews`); return true; });
    const view = mount(true, { unsavedPlan: true, savePlan });
    expect(await screen.findByRole('note')).toHaveTextContent('Unsaved changes in Plan');
    fireEvent.click(screen.getByRole('button', { name: 'Save and preview activation' }));
    await waitFor(() => expect(previewCount()).toBe(1));
    expect(order).toEqual(['save at 0 previews']);

    // An edit after the preview: the preview is of the plan before it.
    expect(screen.queryByRole('button', { name: /Apply to rig databases/ })).not.toBeInTheDocument();
    view.rerender(<Page unsaved={false} save={savePlan} />);
    expect(await screen.findByRole('button', { name: /Apply to rig databases/ })).toBeInTheDocument();
  });

  it('does not preview a plan that could not be saved', async () => {
    const { previewCount } = fixture();
    mount(true, { unsavedPlan: true, savePlan: async () => false });
    fireEvent.click(await screen.findByRole('button', { name: 'Save and preview activation' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('was not saved');
    expect(previewCount()).toBe(0);
  });
});
