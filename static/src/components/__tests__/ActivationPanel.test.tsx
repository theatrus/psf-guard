import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import ActivationPanel from '../director/ActivationPanel';
import { DraftProvider } from '../director/pageDrafts';
import { useDraftSection, usePageDrafts } from '../director/pageDraftsState';
import type { DirectorActivationRemotePush, DirectorActivationReport } from '../../api/directorTypes';

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
const desert = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc';
/** The Desert rig's push to its site, as the activation record keeps it. */
const sitePush = (applied: boolean, error?: string): DirectorActivationRemotePush => ({ rig_id: desert, peer_id: 'obs', peer_name: 'observatory-pi', applied, attempted_at_ms: 1_700_000_100_000, ...(error ? { error } : {}) });
function fixture(conflict = false, activated = false, pushes: DirectorActivationRemotePush[] = [], afterPush: DirectorActivationRemotePush[] = pushes) {
  const applies: Array<{ preview_digest: string }> = [];
  let previews = 0;
  let pushCount = 0;
  server.use(
    http.get('/api/director/v1/projects/project/activation', () => HttpResponse.json(ok({ activation: activated
      ? { project_id: 'project', revision: 2, framing_revision: 2, plan_revision: 3, coordinator_instance_id: 'c', applied_at_ms: 1_700_000_000_000, rigs: [],
        ...((pushCount > 0 ? afterPush : pushes).length > 0 ? { pushes: pushCount > 0 ? afterPush : pushes } : {}) } : null }))),
    http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([{ rig: { id: desert, name: 'Desert', revision: 1 }, catalog_slug: 'desert', catalog_name: 'Desert copy' }]))),
    http.get('/api/director/v1/projects/project/plan', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 },
      plan: { project_id: 'project', revision: 3, updated_at_ms: 1, objectives: [], contributions: [] } }))),
    http.get('/api/director/v1/projects/project/framing', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 },
      draft: { project_id: 'project', revision: 2, layout_revision: 2 } }))),
    http.post('/api/director/v1/projects/project/activation/push', () => { pushCount++; return HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, activation_revision: 2, warnings: [],
      rigs: [{ rig: { id: desert, name: 'Desert', revision: 1 }, catalog_name: 'Desert copy', push: { peer_id: 'obs', peer_name: 'observatory-pi', applied: false, summary: {}, error: 'peer refused the key' } }] })); }),
    http.post('/api/director/v1/projects/project/activation/preview', () => { previews++; return HttpResponse.json(ok(report(false))); }),
    http.post('/api/director/v1/projects/project/activation/apply', async ({ request }) => {
      applies.push(await request.json() as { preview_digest: string });
      if (conflict) return HttpResponse.json({ success: false, data: null, error: 'Director record conflicts with stored content; reload before retrying' }, { status: 409 });
      return HttpResponse.json(ok(report(true)));
    }),
  );
  return { applies, previewCount: () => previews, pushCount: () => pushCount };
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
/** Send once the preview is current: the button waits for it. */
async function send() {
  const button = await screen.findByRole('button', { name: 'Send' });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
}

describe('Activation panel', () => {
  it('sends an assignment with no deadline, and previews again when the assignment changes', async () => {
    const { applies }=fixture();
    let requested: unknown;
    server.use(http.get('/api/director/v1/projects/project/activation/collaboration',()=>HttpResponse.json(ok({imports:[
      {import_id:'assignment',rig_id:rig.id,rig_name:'RedCat',source_digest:'a'.repeat(64),night:'2026-10-05',task_id:'000000000004',version:2,demands:[{panel_index:0,filter:'O',exposure_ms:300000,requested_frames:11}]},
    ]}))),http.post('/api/director/v1/projects/project/activation/preview',async({request})=>{requested=await request.json();return HttpResponse.json(ok(report(false)));}));
    mount();
    const assignment=await screen.findByRole('checkbox',{name:/RedCat.*2026-10-05/});
    expect(within(screen.getByRole('group',{name:/Collaboration assignments/})).getByText('Early beta')).toHaveAttribute('title',expect.stringContaining('Collaboration'));
    // The dialog previewed on opening, without the assignment.
    await waitFor(()=>expect(requested).toEqual({}));
    fireEvent.click(assignment);
    expect(screen.queryByLabelText(/window end/i)).not.toBeInTheDocument();
    // The change outdates the preview at once; a new one follows by itself.
    expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();
    await waitFor(()=>expect(requested).toEqual({collaboration:[{import_id:'assignment',source_digest:'a'.repeat(64)}]}));
    fireEvent.click(assignment);
    expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();
    await waitFor(()=>expect(requested).toEqual({}));
    fireEvent.click(assignment);
    await waitFor(()=>expect(requested).toEqual({collaboration:[{import_id:'assignment',source_digest:'a'.repeat(64)}]}));
    await send();
    await waitFor(()=>expect(applies).toEqual([{preview_digest:'d'.repeat(64),collaboration:[{import_id:'assignment',source_digest:'a'.repeat(64)}]}]));
  });

  it('previews per rig on opening, sends with the preview digest, and reports what landed', async () => {
    const { applies, previewCount } = fixture(); mount();
    // Nothing to send until the preview is in.
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: /preview/i })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Preview ready'));
    expect(previewCount()).toBe(1);
    // Rigs no block shows elsewhere are listed here, each with its counts.
    const redcat = await screen.findByLabelText('Send to RedCat 61');
    expect(redcat).toHaveTextContent('Preview · Project: 1 new · Targets: 2 new · Exposure plans: 2 new');
    expect(screen.getByText(/push it through Sync later/)).toBeInTheDocument();
    expect(screen.getByLabelText('Send to Desert copy')).toHaveTextContent('Will push to Observatory');
    await send();
    expect(await screen.findByText('Pushed to Observatory', { exact: false })).toBeInTheDocument();
    expect(screen.getByLabelText('Send to RedCat 61')).toHaveTextContent('Sent');
    expect(applies).toEqual([{ preview_digest: 'd'.repeat(64) }]);
    expect(screen.getByText(/Revision 1 on the rigs/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Send' })).not.toBeInTheDocument();
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
      { kind: 'plan', action: 'disable', name: 'IC 1805 r1c1 · Ha 300 · 120 s', detail: 'plan #14 (3 of 72 frames taken), replaced by IC 1805 r1c1 · Ha 300 · 300 s' },
    ] }] };
    server.use(http.post('/api/director/v1/projects/project/activation/preview', () => HttpResponse.json(ok(takeover))));
    const rigSummary = await screen.findByLabelText('Send to RedCat 61');
    expect(rigSummary).toHaveTextContent('Preview · Turns off 1 plan · Project: 1 updated');
    expect(rigSummary).toHaveTextContent('Targets: 1 taken over');
    expect(rigSummary).toHaveTextContent('Exposure plans: 1 new, 1 taken over, 1 turned off, 1 left as is');
    // What it turns off stands on its own, open, above the rest.
    const off = within(rigSummary).getByRole('note', { name: 'Turned off in RedCat 61' });
    expect(off).toHaveTextContent('Turns off');
    expect(off).toHaveTextContent('IC 1805 r1c1 · Ha 300 · 120 s plan #14 (3 of 72 frames taken), replaced by IC 1805 r1c1 · Ha 300 · 300 s');
    expect(off).toHaveTextContent("To keep one, take Target Scheduler's values or add it to the plan.");
    const changes = screen.getByText('What changes in RedCat 61').closest('details')!;
    expect(off.compareDocumentPosition(changes) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(changes).toHaveAttribute('open');
    expect(changes).toHaveTextContent('Taken overIC 1805 r1c1 · Ha 300 · 300 s takes over plan #11 (12 of 40 frames taken); desired 40 → 72');
    expect(changes).toHaveTextContent('Left as isIC 1805 r1c1 · Ha 300 · 600 s plan #12');
    expect(changes).not.toHaveTextContent('Turned off');
    expect(changes).toHaveTextContent('Exposure templates');
    expect(changes).toHaveTextContent('New#4 OIII 300');
  });
  it('shows what a preview only turns off, with the details closed and the count once applied', async () => {
    fixture(); mount();
    const offOnly = (applied: boolean): DirectorActivationReport => ({ ...report(applied), rigs: [{ rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile_id: 'p', applied, warnings: [], push: null, changes: [
      { kind: 'project', action: 'unchanged', name: 'Heart', detail: '' },
      { kind: 'plan', action: 'unchanged', name: 'Heart · G_300 · 300 s', detail: '120 frames' },
      { kind: 'plan', action: 'disable', name: 'Heart · G_120 · 120 s', detail: 'plan #34 (2 of 120 frames taken), replaced by Heart · G_300 · 300 s' },
      { kind: 'plan', action: 'disable', name: 'Heart · G_60 · 60 s', detail: 'plan #35 (0 of 10 frames taken), replaced by Heart · G_300 · 300 s' },
    ] }] });
    server.use(
      http.post('/api/director/v1/projects/project/activation/preview', () => HttpResponse.json(ok(offOnly(false)))),
      http.post('/api/director/v1/projects/project/activation/apply', () => HttpResponse.json(ok({ ...offOnly(true), activation_revision: 2 }))),
    );
    const rigSummary = await screen.findByLabelText('Send to RedCat 61');
    expect(rigSummary).toHaveTextContent('Preview · Turns off 2 plans');
    expect(within(rigSummary).getByRole('note', { name: 'Turned off in RedCat 61' })).toHaveTextContent('Heart · G_120 · 120 s plan #34');
    expect(screen.getByText('What changes in RedCat 61').closest('details')).not.toHaveAttribute('open');
    await send();
    // The preview's note stays until the applied record arrives.
    await waitFor(() => expect(screen.getByLabelText('Send to RedCat 61')).toHaveTextContent('Sent · Turned off 2 plans'));
    const applied = screen.getByRole('note', { name: 'Turned off in RedCat 61' });
    expect(applied).toHaveTextContent('Turned off');
    expect(applied).not.toHaveTextContent('To keep one');
  });

  it('says what Target Scheduler shoots afterwards, what it turns off and on, before the other changes', async () => {
    fixture(); mount();
    const faint = '11111111-1111-4111-8111-111111111111';
    const stars = '22222222-2222-4222-8222-222222222222';
    const result = (applied: boolean): DirectorActivationReport => ({ ...report(applied), rigs: [{ rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile_id: 'p', applied, warnings: [], push: null,
      changes: [
        { kind: 'project', action: 'unchanged', name: 'Heart', detail: '' },
        { kind: 'plan', action: 'update', name: 'Heart r1c1 · Ha 300 · 300 s', detail: '102 frames, template #1 Ha 300, summed from 2 objectives; desired 72 → 102' },
        { kind: 'plan', action: 'update', name: 'Heart r2c1 · OIII 300 · 300 s', detail: '72 frames, template #2 OIII 300; turned on' },
        { kind: 'plan', action: 'disable', name: 'Heart r1c1 · SII 300 · 300 s', detail: 'plan #50 (6 of 20 frames taken), not in the plan' },
      ],
      shoots: [
        { plan: 11, target: 'Heart r1c1', template: 'Ha 300', filter: 'Ha', gain: 100, offset: 30, exposure_seconds: 300, desired: 102, acquired: 12, objectives: [faint, stars], turned_on: false },
        { plan: 12, target: 'Heart r2c1', template: 'OIII 300', filter: 'OIII', gain: null, offset: null, exposure_seconds: 300, desired: 72, acquired: 3, objectives: [faint], turned_on: true },
      ] }] });
    server.use(
      http.get('/api/director/v1/projects/project/plan', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 }, plan: { project_id: 'project', revision: 3, updated_at_ms: 1, contributions: [],
        objectives: [{ id: faint, bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'hours', value: 6 }, priority: 1 }, { id: stars, bandpass_id: 'h_alpha', purpose: 'unsaturated_stars', goal: { kind: 'frames', value: 30 }, priority: 1 }] } }))),
      http.post('/api/director/v1/projects/project/activation/preview', () => HttpResponse.json(ok(result(false)))),
      http.post('/api/director/v1/projects/project/activation/apply', () => HttpResponse.json(ok({ ...result(true), activation_revision: 2 }))),
    );
    const rigBlock = await screen.findByLabelText('Send to RedCat 61');
    expect(rigBlock).toHaveTextContent('Preview · Turns off 1 plan · Turns on 1 plan');
    const shoots = within(rigBlock).getByRole('note', { name: 'Shoots in RedCat 61' });
    expect(shoots).toHaveTextContent('Once sent, Target Scheduler shoots');
    await waitFor(() => expect(shoots).toHaveTextContent('Heart r1c1 · 300 s Ha 300 (Ha, gain 100, offset 30) · 102 frames, 12 taken · summed from H-alpha · Faint detail and H-alpha · Unsaturated stars'));
    expect(shoots).toHaveTextContent('Heart r2c1 · 300 s OIII 300 (OIII) · 72 frames, 3 taken');
    const off = within(rigBlock).getByRole('note', { name: 'Turned off in RedCat 61' });
    expect(off).toHaveTextContent('Turns off');
    expect(off).toHaveTextContent('Heart r1c1 · SII 300 · 300 s plan #50 (6 of 20 frames taken), not in the plan');
    expect(off).toHaveTextContent("To keep one, take Target Scheduler's values or add it to the plan.");
    const on = within(rigBlock).getByRole('note', { name: 'Turned on in RedCat 61' });
    expect(on).toHaveTextContent('Turns on');
    expect(on).toHaveTextContent('Heart r2c1 · 300 s OIII 300, plan #12, 3 of 72 frames taken');
    // In that order, all before the folded list.
    const details = screen.getByText('What changes in RedCat 61').closest('details')!;
    for (const [before, after] of [[shoots, off], [off, on], [on, details]]) expect(before.compareDocumentPosition(after) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await send();
    expect(await screen.findByText('Target Scheduler now shoots')).toBeInTheDocument();
    expect(screen.getByRole('note', { name: 'Turned off in RedCat 61' })).toHaveTextContent('Turned off');
    expect(screen.getByRole('note', { name: 'Turned on in RedCat 61' })).toHaveTextContent('Turned on');
    expect(screen.getByLabelText('Send to RedCat 61')).toHaveTextContent('Sent · Turned off 1 plan · Turned on 1 plan');
  });
  it('previews again by itself when Send is refused, and says what changed', async () => {
    fixture(); mount();
    // Someone saved the plan elsewhere meanwhile: Send is refused once, and
    // the next preview has the new plan.
    const applies: string[] = [];
    let previews = 0;
    server.use(http.post('/api/director/v1/projects/project/activation/apply', async ({ request }) => {
      applies.push((await request.json() as { preview_digest: string }).preview_digest);
      return applies.length === 1
        ? HttpResponse.json({ success: false, data: null, error: 'Director record conflicts with stored content; reload before retrying' }, { status: 409 })
        : HttpResponse.json(ok(report(true)));
    }), http.post('/api/director/v1/projects/project/activation/preview', () => {
      previews++;
      const changed = report(false);
      return HttpResponse.json(ok(previews === 1 ? changed : { ...changed, plan_revision: 4, preview_digest: 'e'.repeat(64),
        rigs: [{ ...changed.rigs[0], changes: [...changed.rigs[0].changes, { kind: 'plan', action: 'create', name: 'Heart r1c1 · OIII 300', detail: '72 frames of 300 s' }] }, ...changed.rigs.slice(1)] }));
    }));
    await send();
    const note = await screen.findByText(/^Changed since the preview: the saved plan \(revision 3 → 4\) and RedCat 61's rows\. Nothing was written\./);
    expect(note).toHaveAttribute('role', 'alert');
    expect(note).toHaveTextContent('This preview is new; check it, then send.');
    expect(screen.getByLabelText('Send to RedCat 61')).toHaveTextContent('Exposure plans: 3 new');
    expect(previews).toBe(2);
    // Sending again uses the new preview.
    await send();
    await waitFor(() => expect(applies).toEqual(['d'.repeat(64), 'e'.repeat(64)]));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Sent'));
    expect(previews).toBe(2);
  });

  it('offers to send the last activation only to a site that has not had it, with why and when', async () => {
    const { pushCount } = fixture(false, true, [sitePush(false, 'connection refused')], [sitePush(false, 'peer refused the key')]); mount();
    const unsent = await screen.findByRole('note', { name: 'Not sent to remote sites' });
    await waitFor(() => expect(unsent).toHaveTextContent('Desert copy: not sent to observatory-pi: connection refused · tried'));
    expect(screen.getByText(/^A remote site is behind: observatory-pi hasn't received the last send/)).toBeInTheDocument();
    fireEvent.click(within(unsent).getByRole('button', { name: 'Send to observatory-pi again' }));
    expect(await screen.findByText(/Sent again/)).toBeInTheDocument();
    expect(screen.getByText('Desert copy: Push to observatory-pi failed: peer refused the key')).toBeInTheDocument();
    // The record says how it went, and the block with it.
    await waitFor(() => expect(screen.getByRole('note', { name: 'Not sent to remote sites' })).toHaveTextContent('peer refused the key'));
    expect(pushCount()).toBe(1);
  });
  it('says a push that has not run yet without "again"', async () => {
    fixture(false, true, [{ rig_id: desert, peer_id: 'obs', peer_name: 'observatory-pi', applied: false }]); mount();
    const unsent = await screen.findByRole('note', { name: 'Not sent to remote sites' });
    expect(within(unsent).getByRole('button', { name: 'Send to observatory-pi' })).toBeInTheDocument();
    expect(unsent).not.toHaveTextContent('tried');
  });
  it('offers no send when no rig is remote, or every site has the activation', async () => {
    fixture(false, true); const first = mount();
    expect(await screen.findByText(/^Last sent /)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^(Send to|Push)/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('note', { name: 'Not sent to remote sites' })).not.toBeInTheDocument();
    first.unmount();
    fixture(false, true, [sitePush(true)]); mount();
    expect(await screen.findByText(/^Last sent /)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^(Send to|Push)/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/hasn't received/)).not.toBeInTheDocument();
  });

  it("takes a rig's Target Scheduler values instead of overwriting them, then previews again", async () => {
    const takes: unknown[] = [];
    let previews = 0;
    const differing = (same: boolean): DirectorActivationReport => ({
      ...report(false),
      rigs: [
        { rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile_id: 'p', applied: false, warnings: [], push: null, changes: [
          { kind: 'project', action: 'unchanged', name: 'Heart', detail: '' },
          { kind: 'plan', action: same ? 'unchanged' : 'update', name: 'Heart r1c1 · Ha 300', detail: same ? '90 frames' : 'desired 90 → 72' },
        ] },
        // A rig whose project activation would create has nothing to take.
        { rig: { id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc', name: 'Desert', revision: 1 }, catalog_slug: 'desert', catalog_name: 'Desert copy', profile_id: 'p', applied: false, warnings: [], push: null,
          changes: [{ kind: 'project', action: 'create', name: 'Heart', detail: 'new Target Scheduler project' }] },
      ],
    });
    server.use(
      http.get('/api/director/v1/projects/project/activation', () => HttpResponse.json(ok({ activation: null }))),
      http.get('/api/director/v1/projects/project/plan', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 },
        plan: { project_id: 'project', revision: 3, updated_at_ms: 1, objectives: [], contributions: [] } }))),
      http.get('/api/director/v1/projects/project/framing', () => HttpResponse.json(ok({ project: { id: 'project', name: 'Heart', revision: 1 },
        draft: { project_id: 'project', revision: 2, layout_revision: 2 } }))),
      http.post('/api/director/v1/projects/project/activation/preview', () => { previews++; return HttpResponse.json(ok(differing(takes.length > 0))); }),
      http.post('/api/director/v1/projects/project/plan/take-target-scheduler', async ({ request }) => {
        takes.push(await request.json());
        return HttpResponse.json(ok({ plan_revision: 4, framing_revision: 2, taken: ['Ha: 90 frames'],
          left: ['Project: a Draft in Target Scheduler; sending to the rigs makes it Active'] }));
      }),
    );
    mount();
    const take = await screen.findByRole('button', { name: "Take Target Scheduler's values" });
    expect(screen.getAllByRole('button', { name: "Take Target Scheduler's values" })).toHaveLength(1);
    await waitFor(() => expect(take).toBeEnabled());
    const before = previews;
    fireEvent.click(take);
    expect(await screen.findByText('Ha: 90 frames')).toBeInTheDocument();
    expect(screen.getByText('Project: a Draft in Target Scheduler; sending to the rigs makes it Active')).toBeInTheDocument();
    expect(takes).toEqual([{ rig_id: rig.id, plan_revision: 3, framing_revision: 2 }]);
    // The preview runs again on what is saved now, and has nothing to take.
    await waitFor(() => expect(previews).toBeGreaterThan(before));
    await waitFor(() => expect(screen.queryByRole('button', { name: "Take Target Scheduler's values" })).not.toBeInTheDocument());
  });

  it('offers nothing to a read-only account, and previews nothing for it', async () => {
    const { previewCount } = fixture(); mount(false);
    expect(await screen.findByText('Read only')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Send' })).not.toBeInTheDocument();
    await new Promise(resolve => setTimeout(resolve, 400));
    expect(previewCount()).toBe(0);
  });
  it('asks to save unsaved edits first, never saves unasked, and previews once they are saved', async () => {
    // Sending writes the saved plan: a goal raised in the editor but not
    // saved would otherwise be sent as it was before, and look lost.
    const { previewCount } = fixture();
    const savePlan = vi.fn(async () => true);
    const view = mount(true, { unsavedPlan: true, savePlan });
    const note = await screen.findByRole('note', { name: 'Unsaved changes' });
    expect(note).toHaveTextContent('Unsaved changes in Plan. The rigs get the saved plan, so save first; the preview follows.');
    // No preview of a plan about to change, and nothing to send.
    await new Promise(resolve => setTimeout(resolve, 400));
    expect(previewCount()).toBe(0);
    expect(savePlan).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Send' })).not.toBeInTheDocument();
    fireEvent.click(within(note).getByRole('button', { name: 'Save and send…' }));
    await waitFor(() => expect(savePlan).toHaveBeenCalledTimes(1));
    expect(previewCount()).toBe(0);
    // Saved: the page's draft clears, and the preview follows by itself.
    view.rerender(<Page unsaved={false} save={savePlan} />);
    await waitFor(() => expect(previewCount()).toBe(1));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled());
    expect(screen.queryByRole('note', { name: 'Unsaved changes' })).not.toBeInTheDocument();
  });

  it('does not preview a plan that could not be saved', async () => {
    const { previewCount } = fixture();
    mount(true, { unsavedPlan: true, savePlan: async () => false });
    fireEvent.click(await screen.findByRole('button', { name: 'Save and send…' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('was not saved');
    expect(previewCount()).toBe(0);
  });

  it('says a failed preview, and retries it on Refresh', async () => {
    fixture(); mount();
    let fail = true;
    server.use(http.post('/api/director/v1/projects/project/activation/preview', () => fail
      ? HttpResponse.json({ success: false, data: null, error: 'RedCat 61 is locked' }, { status: 500 })
      : HttpResponse.json(ok(report(false)))));
    expect(await screen.findByRole('alert')).toHaveTextContent('RedCat 61 is locked');
    expect(screen.getByRole('status')).toHaveTextContent('Preview failed');
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Send' })).toHaveAccessibleDescription('No preview to send.');
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Preview ready'));
    expect(screen.queryByRole('button', { name: 'Refresh' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
  });

  it('says it is previewing, outside the busy preview, and why Send waits', async () => {
    fixture(); mount();
    let release: () => void = () => {};
    server.use(http.post('/api/director/v1/projects/project/activation/preview', async () => {
      await new Promise<void>(resolve => { release = resolve; });
      return HttpResponse.json(ok(report(false)));
    }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Previewing…'));
    expect(screen.getByRole('status').closest('[aria-busy="true"]')).toBeNull();
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Send' })).toHaveAccessibleDescription('Waiting for the preview.');
    release();
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Preview ready'));
    expect(screen.getByRole('region', { name: 'Preview' })).toHaveAttribute('aria-busy', 'false');
    expect(screen.getByRole('button', { name: 'Send' })).not.toHaveAccessibleDescription();
  });

  it('sends once a matching preview reconciles, and on a refusal says what Send found, previewing once more', async () => {
    fixture(); mount();
    // Send found a row N.I.N.A. changed after the preview read its copy.
    const found: DirectorActivationReport = { ...report(false), preview_digest: 'f'.repeat(64),
      rigs: [{ ...report(false).rigs[0], warnings: ['Its database stayed locked (N.I.N.A. or a refresh may be writing it); try again in a moment.'] }, ...report(false).rigs.slice(1)] };
    let previews = 0;
    let release: () => void = () => {};
    const applies: string[] = [];
    server.use(http.post('/api/director/v1/projects/project/activation/apply', async ({ request }) => {
      applies.push((await request.json() as { preview_digest: string }).preview_digest);
      return applies.length === 1
        ? HttpResponse.json({ success: false, data: found, error: 'The plan or a rig database changed since the preview; nothing was written. Preview again.' }, { status: 409 })
        : HttpResponse.json(ok(report(true)));
    }), http.post('/api/director/v1/projects/project/activation/preview', async () => {
      previews++;
      if (previews === 2) await new Promise<void>(resolve => { release = resolve; });
      return HttpResponse.json(ok(previews === 1 ? report(false) : found));
    }));
    await send();
    // What Send found is said at once; the new preview is on its way.
    const note = await screen.findByRole('alert');
    expect(note).toHaveTextContent("Changed since the preview: RedCat 61: Its database stayed locked (N.I.N.A. or a refresh may be writing it); try again in a moment. Nothing was written. Previewing again…");
    await waitFor(() => expect(previews).toBe(2));
    // Send stays off until it lands, never flickering on in between.
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.getByRole('region', { name: 'Preview' })).toHaveAttribute('aria-busy', 'true');
    release();
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('This preview is new; check it, then send.'));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled());
    await new Promise(resolve => setTimeout(resolve, 500));
    expect(previews).toBe(2);
    await send();
    await waitFor(() => expect(applies).toEqual(['d'.repeat(64), 'f'.repeat(64)]));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Sent'));
  });

  it('says what a plan needs before it can be sent, as an instruction, with no Refresh', async () => {
    fixture();
    const onFraming = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    server.use(http.post('/api/director/v1/projects/project/activation/preview', () =>
      HttpResponse.json({ success: false, data: null, error: 'Choose a panel rig or enter a panel size in Framing first.' }, { status: 422 })));
    render(<QueryClientProvider client={client}><ActivationPanel projectId="project" onFraming={onFraming} /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Not ready to send: Choose a panel rig or enter a panel size in Framing first.'));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByText(/failed/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Refresh' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send' })).toHaveAccessibleDescription('Nothing to send until then.');
    fireEvent.click(screen.getByRole('button', { name: 'Open Framing' }));
    expect(onFraming).toHaveBeenCalledTimes(1);
  });

  it('refreshes what the page shows after a send though the dialog closed while it ran', async () => {
    fixture();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const invalidated = vi.spyOn(client, 'invalidateQueries');
    let release: () => void = () => {};
    server.use(http.post('/api/director/v1/projects/project/activation/apply', async () => {
      await new Promise<void>(resolve => { release = resolve; });
      return HttpResponse.json(ok(report(true)));
    }));
    const view = render(<QueryClientProvider client={client}><ActivationPanel projectId="project" /></QueryClientProvider>);
    await send();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Sending…' })).toBeInTheDocument());
    // Escape, mid-send.
    view.unmount();
    invalidated.mockClear();
    release();
    await waitFor(() => expect(invalidated.mock.calls.map(([filters]) => filters?.queryKey)).toEqual(expect.arrayContaining([
      ['directorActivation', 'project'], ['directorActivationCheck', 'project'], ['db'], ['directorPlans'],
    ])));
  });

  it('keeps focus in the dialog once Save and send… has saved', async () => {
    fixture();
    const savePlan = vi.fn(async () => true);
    const view = mount(true, { unsavedPlan: true, savePlan });
    const save = await screen.findByRole('button', { name: 'Save and send…' });
    save.focus();
    fireEvent.click(save);
    await waitFor(() => expect(savePlan).toHaveBeenCalledTimes(1));
    view.rerender(<Page unsaved={false} save={savePlan} />);
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('status')));
    expect(document.activeElement).not.toBe(document.body);
  });
});
