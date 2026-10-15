import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import ProjectWorkspace from '../director/ProjectWorkspace';
import type { DirectorActivationRemotePush } from '../../api/directorTypes';

vi.mock('../ProjectSchedulerDialog', () => ({
  ProjectPlanEditor: ({ dbId, projectId }: { dbId: string; projectId: number }) => <output>{`Source editor ${dbId}:${projectId}`}</output>,
}));
vi.mock('../director/FramingView', () => ({
  default: ({ projectId, seed }: { projectId: string; seed: { name: string; center: { ra_degrees: number } } | null }) => <output>{`Framing ${projectId}:${seed ? `${seed.name}@${seed.center.ra_degrees}` : 'no seed'}`}</output>,
}));
// The editor keeps which rigs shoot the plan; the stub does the same, lets
// the Rigs tab add and drop them through `controls`, and holds one draft
// field so the save bar and the tabs have an edit to track.
vi.mock('../director/PlanEditor', async () => {
  const { useEffect, useState } = await vi.importActual<typeof import('react')>('react');
  const { useDraftSection } = await vi.importActual<typeof import('../director/pageDraftsState')>('../director/pageDraftsState');
  function Goal() {
    const [goal, setGoal] = useState('40');
    useDraftSection('plan', { label: 'Exposures', order: 2, unsaved: goal !== '40', save: async () => true, discard: () => setGoal('40') });
    return <input aria-label="Stub goal" value={goal} onChange={event => setGoal(event.target.value)} />;
  }
  function StubPlanEditor({ projectId, controls, onRigsChange }: {
    projectId: string;
    controls?: { current: { setRig: (id: string, on: boolean) => { done: boolean; note: string | null } } | null };
    onRigsChange?: (state: { rigIds: string[]; objectives: number; ready: boolean }) => void;
  }) {
    const [joined, setJoined] = useState<string[]>([]);
    useEffect(() => { if (controls) controls.current = { setRig: (id, on) => { setJoined(ids => on ? [...ids, id] : ids.filter(other => other !== id)); return { done: true, note: null }; } }; });
    useEffect(() => { onRigsChange?.({ rigIds: joined, objectives: 1, ready: true }); }, [joined, onRigsChange]);
    return <div><output>{`Plan ${projectId}: ${joined.join(', ') || 'no rigs'}`}</output><Goal /></div>;
  }
  return { default: StubPlanEditor };
});
vi.mock('../director/ActivationPanel', () => ({ default: ({ projectId }: { projectId: string }) => <output>{`Activation ${projectId}`}</output> }));
vi.mock('../director/ObservingPreferences', () => ({ default: () => <output>Observing preferences</output> }));
const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
function Where() { return <output data-testid="where">{useLocation().search}</output>; }
/** The saved framing, plan and last activation the summary reads. */
function summaryHandlers(saved: { framing?: boolean; framingRevision?: number; layoutRevision?: number; planRevision?: number; shoots?: boolean; activated?: { plan_revision: number; rigs?: string[]; pushes?: () => DirectorActivationRemotePush[] }; databaseManagement?: boolean; matches?: boolean; plans?: Array<{ action: string; name: string; detail: string }>; checkAfter?: Promise<void>; check?: 'fails' | 'unreadable' } = {}) {
  return [
    http.get('/api/director/v1/projects/project/framing', () => ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, draft: saved.framing ? {
      project_id: 'project', revision: saved.framingRevision ?? 2, layout_revision: saved.layoutRevision, target_name: 'M31', center: { ra_degrees: 10.68, dec_degrees: 41.27 }, position_angle_degrees: 0, mosaic: { rows: 1, columns: 1, overlap_percent: 20 },
      panel_rig_id: null, panel: null, shown_rig_ids: [], survey_id: 'dss2', view_fov_degrees: 4, updated_at_ms: 1, rig_framings: [] } : null })),
    http.get('/api/director/v1/projects/project/plan', () => ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, plan: saved.planRevision === undefined ? null : {
      project_id: 'project', revision: saved.planRevision, updated_at_ms: 1, contributions: saved.shoots ? [{ id: 'c1', rig_id: 'rig-catalog', objective_id: 'o1', enabled: true, exposure_seconds: 300, panel_ids: [], template: { template_guid: 't', template_id: 1, name: 'Ha', filter_name: 'Ha', gain: null, offset: null, bin: 1, readout_mode: null } }] : [],
      objectives: [{ id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'frames', value: 40 }, priority: 1 }] } })),
    // The rigs the activation reached: by default each rig the plan shoots.
    http.get('/api/director/v1/projects/project/activation', () => ok({ activation: saved.activated ? { project_id: 'project', revision: 3, framing_revision: 2, plan_revision: saved.activated.plan_revision,
      coordinator_instance_id: 'c', applied_at_ms: 1, rigs: (saved.activated.rigs ?? (saved.shoots ? ['rig-catalog'] : [])).map(id => ({ rig_id: id, catalog_id: 'x', project_guid: 'guid', profile_id: 'p', targets: [], plans: [] })),
      ...(saved.activated.pushes ? { pushes: saved.activated.pushes() } : {}) } : null })),
    http.get('/api/director/v1/status', () => ok({ protocol_version: 1, enabled: true, instance_id: 'instance', database_management: saved.databaseManagement ?? true })),
    // What an activation would change, from a copy of each rig's rows: by
    // default the rig lacks the plan; `matches` holds it already, but for
    // any exposure `plans` rows. `checkAfter` holds the answer back; `check`
    // fails it, or reports the rig's database unreadable.
    http.get('/api/director/v1/projects/project/activation/check', async () => {
      await saved.checkAfter;
      if (saved.check === 'fails') return HttpResponse.json({ success: false, data: null, error: 'The check failed: disk I/O error' }, { status: 500 });
      const unreadable = saved.check === 'unreadable';
      return ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, framing_revision: 2, plan_revision: saved.planRevision ?? 1, panels: 1,
        warnings: [], preview_digest: '0'.repeat(64), applied: false, activation_revision: null,
        rigs: (saved.shoots ? ['rig-catalog'] : []).map(id => ({ rig: { id, name: 'C925', revision: 1 }, catalog_slug: 'catalog', catalog_name: 'C925 data', profile_id: unreadable ? null : 'profile',
          warnings: unreadable ? ['Its database could not be read: file is not a database.'] : [], applied: false, push: null,
          changes: unreadable ? [] : [{ kind: 'project', action: saved.matches ? 'unchanged' : 'create', name: 'Andromeda', detail: '' }, ...(saved.plans ?? []).map(plan => ({ kind: 'plan', ...plan }))] })) });
    }),
  ];
}
const rig = { id: 'rig', name: 'C925', revision: 1 };

function mount(links: Array<{ catalog_slug: string; catalog_name: string; source_row_id: number | null; source_name: string | null }>, route = '/plan?db=catalog&plan=project', saved: Parameters<typeof summaryHandlers>[0] = { framing: true }) {
  server.use(
    ...summaryHandlers(saved),
    // Every database is a rig of its own.
    http.get('/api/director/v1/rigs/profiles', () => ok(links.map(link => ({ rig: { ...rig, id: `rig-${link.catalog_slug}` }, catalog_slug: link.catalog_slug, catalog_name: link.catalog_name })))),
    http.get('/api/director/v1/plans', () => ok({ warnings: [], rows: [{ project: { id: 'project', name: 'Andromeda', revision: 1 }, framing: null, plan: null, activation: null,
      links: links.map(link => ({ ...link, rig: { ...rig, id: `rig-${link.catalog_slug}` }, source_project_guid: 'guid' })) }] })),
    http.get('/api/db/catalog/projects/7/scheduler', () => ok({ id: 7, name: 'Andromeda subs', exposure_templates: [], targets: [{ id: 1, name: 'M31', active: true, ra_hours: 0.5, dec_degrees: 41.27, epoch_code: 2, rotation: 35, roi: 100, exposure_plans: [] }] })),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>
    <ProjectWorkspace instanceId="instance" projectId="project" />
    <Where />
  </MemoryRouter></QueryClientProvider>);
}

describe('project workspace', () => {
  const links = [{ catalog_slug: 'catalog', catalog_name: 'C925 data', source_row_id: 7, source_name: 'Andromeda subs' }, { catalog_slug: 'redcat', catalog_name: 'Redcat data', source_row_id: null, source_name: null }];
  it('seeds framing from the first linked database and lists each rig with its Target Scheduler project on the Rigs tab', async () => {
    mount(links);
    expect(await screen.findByRole('heading', { name: 'Andromeda' })).toBeInTheDocument();
    expect(await screen.findByText('Framing project:M31@7.5')).toBeInTheDocument();
    expect(screen.getByText('Plan project: no rigs')).toBeInTheDocument();
    // Activation is no tab; the editors load once the Rigs tab is opened, the arrival database first.
    expect(screen.queryByRole('tab', { name: 'Activate' })).not.toBeInTheDocument();
    expect(screen.queryByText('Source editor catalog:7')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: 'Rigs' }));
    const rigs = screen.getByRole('region', { name: 'Rigs' });
    expect(within(rigs).getByText('Source editor catalog:7')).toBeInTheDocument();
    expect(within(rigs).getByText(/opened from here/)).toBeInTheDocument();
    // A database whose project row is gone has nothing to edit until activation makes one.
    expect(within(rigs).getByRole('group', { name: 'Redcat data' })).toHaveTextContent('Created on activation');
    fireEvent.click(within(rigs).getByRole('button', { name: /Target Scheduler settings/ }));
    expect(screen.queryByText('Source editor catalog:7')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Library' })).toHaveAttribute('href', '/?db=catalog');
  });
  it('keeps the tab in the address and opens the one a link names', async () => {
    mount(links, '/plan?plan=project&planTab=priority');
    expect(await screen.findByRole('tab', { name: 'Priority and defaults' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText('Observing preferences').closest('[role="tabpanel"]')).not.toHaveAttribute('hidden');
    expect(screen.getByText(/^Plan project/).closest('[role="tabpanel"]')).toHaveAttribute('hidden');
    fireEvent.click(screen.getByRole('tab', { name: 'Exposures' }));
    expect(screen.getByRole('tab', { name: 'Exposures' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('where')).toHaveTextContent('planTab=exposures');
    expect(screen.getByTestId('where')).toHaveTextContent('plan=project');
    // Arrow keys move along the tabs.
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Exposures' }), { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: 'Rigs' })).toHaveAttribute('aria-selected', 'true');
  });
  it('opens the tab an older link names under its new name', async () => {
    mount(links, '/plan?plan=project&planTab=databases');
    expect(await screen.findByRole('tab', { name: 'Rigs' })).toHaveAttribute('aria-selected', 'true');
  });
  it('keeps an unsaved edit across tabs and marks the tab that holds it', async () => {
    mount(links, '/plan?plan=project&planTab=plan');
    const goal = await screen.findByLabelText('Stub goal');
    fireEvent.change(goal, { target: { value: '120' } });
    // The dot is for the eye; a screen reader hears "edited" in the tab's name.
    expect(screen.getByRole('tab', { name: 'Exposures, edited' })).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Unsaved changes' })).toHaveTextContent('Unsaved changes in Exposures.');
    fireEvent.click(screen.getByRole('tab', { name: 'Framing' }));
    fireEvent.click(screen.getByRole('tab', { name: /Exposures/ }));
    expect(screen.getByLabelText('Stub goal')).toHaveValue('120');
  });
  it('asks for an activation at the top once the saved plan is not on the rigs, and opens the preview', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 1, shoots: true });
    const due = await screen.findByRole('region', { name: 'Activation due' });
    expect(due).toHaveTextContent('Not synced: not activated yet');
    // No rig is remote, so there is nothing to send.
    expect(within(due).queryByRole('button', { name: /^Send/ })).not.toBeInTheDocument();
    // Said once: the summary leaves it to the bar.
    expect(screen.queryByTestId('summary-activation')).not.toBeInTheDocument();
    expect(screen.queryByText('Activation project')).not.toBeInTheDocument();
    fireEvent.click(within(due).getByRole('button', { name: 'Activate…' }));
    expect(within(screen.getByRole('dialog', { name: 'Activate on the rigs' })).getByText('Activation project')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /close/i }));
    // Unsaved edits come first: the save bar takes the top until they are saved.
    fireEvent.change(screen.getByLabelText('Stub goal'), { target: { value: '120' } });
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Unsaved changes' })).toBeInTheDocument();
  });
  it('does not ask for an activation when only the view of the framing changed', async () => {
    // Revision 5 changed the survey; the layout the rigs have is still revision 2's.
    mount(links, '/plan?plan=project', { framing: true, framingRevision: 5, layoutRevision: 2, planRevision: 4, shoots: true, activated: { plan_revision: 4 }, matches: true });
    await waitFor(() => expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Synced$/));
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
  });
  it('asks for one when the layout moved after the activation', async () => {
    mount(links, '/plan?plan=project', { framing: true, framingRevision: 5, layoutRevision: 5, planRevision: 4, shoots: true, activated: { plan_revision: 4 } });
    expect(await screen.findByRole('region', { name: 'Activation due' })).toHaveTextContent('Not synced: saved framing not activated yet');
  });
  it('does not ask for an activation the rigs already have', async () => {
    // A plan the rig holds beside one the plan leaves alone (`keep`) is in step.
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4 }, matches: true,
      plans: [{ action: 'unchanged', name: 'M31 · Ha · 300 s', detail: '' }, { action: 'keep', name: 'M31 · OIII · 600 s', detail: 'not part of this plan; left as it is' }] });
    // Not "Active", which is a Target Scheduler project state of its own.
    const summary = await screen.findByRole('region', { name: 'Plan summary' });
    await waitFor(() => expect(within(summary).getByTestId('summary-activation')).toHaveTextContent(/^Synced$/));
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    // Where it stands is no button; the button beside it opens the
    // activation, to push it again.
    expect(within(summary).getByTestId('summary-activation').tagName).toBe('SPAN');
    fireEvent.click(within(summary).getByRole('button', { name: 'Activate…' }));
    expect(screen.getByRole('dialog', { name: 'Activate on the rigs' })).toBeInTheDocument();
  });
  it('asks again when the record is up to date but activation would still change a rig\'s rows', async () => {
    // SNR G132: the record has the saved plan, but the rig still runs the
    // G_120 plan the G_300 one replaced, and activation would turn it off.
    let answer = () => {};
    const checkAfter = new Promise<void>(resolve => { answer = resolve; });
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4 }, matches: true, checkAfter,
      plans: [{ action: 'unchanged', name: 'SNR G132 · G_300 · 300 s', detail: '' }, { action: 'disable', name: 'SNR G132 · G_120 · 120 s', detail: 'plan #34 (2 of 120 frames taken) is replaced by SNR G132 · G_300 · 300 s' }] });
    // The record answers at once while the check runs, and nothing asks yet.
    await waitFor(() => expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Synced$/));
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    answer();
    const due = await screen.findByRole('region', { name: 'Activation due' });
    // Turning a plan off is all it would do, and the bar says so.
    expect(due).toHaveTextContent('Not synced: turns off 1 plan in C925 data');
    expect(screen.queryByTestId('summary-activation')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: /Rigs/ }));
    expect(within(await screen.findByRole('group', { name: 'C925 data' })).getByText(/^not synced · turns off 1 plan/)).toBeInTheDocument();
    fireEvent.click(within(due).getByRole('button', { name: 'Activate again…' }));
    expect(screen.getByRole('dialog', { name: 'Activate on the rigs' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /close/i }));
    // With edits unsaved the save bar comes first; the summary says it,
    // short, with why in its tooltip.
    fireEvent.click(screen.getByRole('tab', { name: /Exposures/ }));
    fireEvent.change(screen.getByLabelText('Stub goal'), { target: { value: '120' } });
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Not synced$/);
    expect(screen.getByTestId('summary-activation')).toHaveAttribute('title', 'Not synced: turns off 1 plan in C925 data');
  });
  it('says a rig differs from the plan when activation would change more than turn plans off', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4 }, matches: true,
      plans: [{ action: 'update', name: 'M31 · Ha · 300 s', detail: '40 frames; desired 100 → 40' }, { action: 'disable', name: 'M31 · Ha · 120 s', detail: 'plan #9 (0 of 10 frames taken), replaced by M31 · Ha · 300 s' }] });
    expect(await screen.findByRole('region', { name: 'Activation due' })).toHaveTextContent(/^Not synced: C925 data differs from the plan/);
    fireEvent.click(screen.getByRole('tab', { name: /Rigs/ }));
    expect(within(await screen.findByRole('group', { name: 'C925 data' })).getByText(/^not synced · differs from the plan/)).toBeInTheDocument();
  });
  it('asks again for a rig the last activation left out, though the revisions match', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4, rigs: [] } });
    expect(await screen.findByRole('region', { name: 'Activation due' })).toHaveTextContent('Not synced: C925 data not activated yet');
    expect(screen.queryByTestId('summary-activation')).not.toBeInTheDocument();
  });
  it('says a remote site that missed the activation is not synced, and sends it there again', async () => {
    // The rows here match the plan, but observatory-pi was offline at Apply.
    let sent = false;
    const posts: string[] = [];
    const push = (): DirectorActivationRemotePush[] => [{ rig_id: 'rig-catalog', peer_id: 'obs', peer_name: 'observatory-pi', applied: sent, attempted_at_ms: 1_700_000_100_000, ...(sent ? {} : { error: 'connection refused' }) }];
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4, pushes: push }, matches: true });
    server.use(http.post('/api/director/v1/projects/project/activation/push', () => { posts.push('push'); sent = true;
      return ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, activation_revision: 3, warnings: [],
        rigs: [{ rig: { id: 'rig-catalog', name: 'C925', revision: 1 }, catalog_name: 'C925 data', push: { peer_id: 'obs', peer_name: 'observatory-pi', applied: true, summary: {}, error: null } }] }); }));
    const due = await screen.findByRole('region', { name: 'Activation due' });
    expect(due).toHaveTextContent("Not synced: observatory-pi hasn't received this activation");
    // The rows match, so the send comes first; Activate… still opens the
    // dialog, since the summary's button hides while the bar is up.
    expect(within(due).getByRole('button', { name: 'Send to observatory-pi again' })).toHaveAttribute('title', 'connection refused');
    expect(within(due).getByRole('button', { name: 'Send to observatory-pi again' })).toHaveClass('is-primary');
    expect(within(due).getByRole('button', { name: 'Activate…' })).not.toHaveClass('is-primary');
    expect(screen.queryByTestId('summary-activation')).not.toBeInTheDocument();
    // The rig's line says where it was not sent.
    fireEvent.click(screen.getByRole('tab', { name: /Rigs/ }));
    expect(within(await screen.findByRole('group', { name: 'C925 data' })).getByText(/^not synced · not sent to observatory-pi/)).toHaveAttribute('title', 'connection refused');
    // With edits unsaved the save bar comes first, and the summary says it.
    fireEvent.click(screen.getByRole('tab', { name: /Exposures/ }));
    fireEvent.change(screen.getByLabelText('Stub goal'), { target: { value: '120' } });
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Not synced$/);
    expect(screen.getByTestId('summary-activation')).toHaveAttribute('title', "Not synced: observatory-pi hasn't received this activation");
    fireEvent.change(screen.getByLabelText('Stub goal'), { target: { value: '40' } });
    fireEvent.click(within(await screen.findByRole('region', { name: 'Activation due' })).getByRole('button', { name: 'Send to observatory-pi again' }));
    // Sent: the record says so, and the plan reads Synced.
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Synced$/));
    expect(posts).toEqual(['push']);
  });
  it('offers both an activation and the send when the rows differ and a site missed the last one', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 5, shoots: true, activated: { plan_revision: 4, pushes: () => [{ rig_id: 'rig-catalog', peer_id: 'obs', peer_name: 'observatory-pi', applied: false }] } });
    const due = await screen.findByRole('region', { name: 'Activation due' });
    // The missed send shows first; the plan's revision follows.
    await waitFor(() => expect(due).toHaveTextContent("Not synced: saved plan not activated yet; observatory-pi hasn't received this activation"));
    expect(within(due).getByRole('button', { name: 'Send to observatory-pi' })).not.toHaveClass('is-primary');
    expect(within(due).getByRole('button', { name: 'Activate again…' })).toHaveClass('is-primary');
  });
  it('says the rigs could not be checked, without asking, when the check fails', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4 }, check: 'fails' });
    const summary = await screen.findByRole('region', { name: 'Plan summary' });
    await waitFor(() => expect(within(summary).getByTestId('summary-activation')).toHaveTextContent(/^Couldn't check the rigs$/));
    expect(within(summary).getByTestId('summary-activation')).toHaveAttribute('title', 'The check failed: disk I/O error');
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    expect(within(summary).getByRole('button', { name: 'Activate…' })).toBeInTheDocument();
  });
  it('says which rig could not be checked when its database could not be read', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4 }, check: 'unreadable' });
    await waitFor(() => expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Couldn't check C925 data$/));
    expect(screen.getByTestId('summary-activation')).toHaveAttribute('title', 'Its database could not be read: file is not a database.');
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: /Rigs/ }));
    expect(within(await screen.findByRole('group', { name: 'C925 data' })).getByText(/^couldn't check/)).toHaveAttribute('title', 'Its database could not be read: file is not a database.');
  });
  it('does not ask for an activation when the rigs already hold a plan none was recorded for', async () => {
    // A plan taken in from Target Scheduler: the rows it came from match it.
    mount(links, '/plan?plan=project', { framing: true, planRevision: 1, shoots: true, matches: true });
    await waitFor(() => expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Synced$/));
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: /Rigs/ }));
    expect(within(await screen.findByRole('group', { name: 'C925 data' })).getByText(/^synced/)).toBeInTheDocument();
  });
  it('does not ask when the saved plan is newer than the record but the rows already match it', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 5, shoots: true, activated: { plan_revision: 4 }, matches: true });
    await waitFor(() => expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Synced$/));
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
  });
  it('leaves the activation bar out on a server that does not write rig databases', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 1, shoots: true, databaseManagement: false });
    expect(await screen.findByTestId('summary-drafts-only')).toHaveTextContent('Drafts only');
    // The summary still says where it stands and opens the preview.
    await waitFor(() => expect(screen.getByTestId('summary-activation')).toHaveTextContent(/^Not synced$/));
    expect(screen.getByTestId('summary-activation')).toHaveAttribute('title', 'Not synced: not activated yet');
    expect(within(screen.getByRole('region', { name: 'Plan summary' })).getByRole('button', { name: 'Activate…' })).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
  });
  it('sums the plan up once: target, goals, each rig and whether it is ready, and the activation', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 5, activated: { plan_revision: 4 } });
    expect(await screen.findByTestId('summary-target')).toHaveTextContent('M31');
    expect(await screen.findByTestId('summary-goals')).toHaveTextContent('H-alpha · 40 frames per rig');
    const summary = await screen.findByRole('region', { name: 'Plan summary' });
    expect(await within(summary).findByText('C925 data')).toBeInTheDocument();
    expect(within(summary).getAllByText(/no rig profile/).length).toBe(2);
    // Both databases hold a project, but neither rig shoots the plan now.
    expect(within(summary).getAllByText(/· off/).length).toBe(2);
    expect(await screen.findByTestId('summary-activation')).toHaveTextContent(/^Not synced$/);
    expect(screen.getByTestId('summary-activation')).toHaveAttribute('title', 'Not synced: saved plan not activated yet');
  });
  it('opens on framing, saved or not, unless the address names a tab', async () => {
    mount(links, '/plan?plan=project', { framing: false });
    expect(await screen.findByRole('tab', { name: 'Framing' })).toHaveAttribute('aria-selected', 'true');
  });
  it('opens on framing when a framing is saved too', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 1 });
    expect(await screen.findByTestId('summary-target')).toHaveTextContent('M31');
    expect(screen.getByRole('tab', { name: 'Framing' })).toHaveAttribute('aria-selected', 'true');
  });
  it('frames an unlinked project without a seed', async () => {
    mount([]);
    expect(await screen.findByText('Framing project:no seed')).toBeInTheDocument();
    expect(screen.getByText('No rigs yet')).toBeInTheDocument();
  });
});

describe('attaching and detaching database projects', () => {
  const rigA = { id: 'rig-a', name: 'C925', revision: 1 };
  const rigB = { id: 'rig-b', name: 'RedCat', revision: 1 };
  function mountTwo() {
    const posts: Array<{ url: string; body: unknown }> = [];
    server.use(
      ...summaryHandlers({ framing: true }),
      http.get('/api/director/v1/rigs/profiles', () => ok([
        { rig: rigA, catalog_slug: 'catalog', catalog_name: 'C925 data' },
        { rig: { id: 'rig-c', name: 'Third', revision: 1 }, catalog_slug: 'third', catalog_name: 'Third data' },
        { rig: { id: 'rig-s', name: 'Spare', revision: 1 }, catalog_slug: 'spare', catalog_name: 'Spare data' },
      ])),
      http.get('/api/director/v1/plans', () => ok({ warnings: [], rows: [
        { project: { id: 'project', name: 'Heart', revision: 1 }, framing: null, plan: null, activation: null,
          links: [{ catalog_slug: 'catalog', catalog_name: 'C925 data', rig: rigA, source_project_guid: 'guid-a', source_row_id: 7, source_name: 'Heart' }, { catalog_slug: 'third', catalog_name: 'Third data', rig: { id: 'rig-c', name: 'Third', revision: 1 }, source_project_guid: 'guid-c', source_row_id: 9, source_name: 'Heart' }] },
        { project: { id: 'other', name: 'Heart by RedCat', revision: 1 }, framing: null, plan: null, activation: null,
          links: [{ catalog_slug: 'redcat', catalog_name: 'Redcat data', rig: rigB, source_project_guid: 'guid-b', source_row_id: 3, source_name: 'Heart by RedCat' }] },
        { project: { id: 'same-db', name: 'Also on C925', revision: 1 }, framing: null, plan: null, activation: null,
          links: [{ catalog_slug: 'catalog', catalog_name: 'C925 data', rig: rigA, source_project_guid: 'guid-d', source_row_id: 8, source_name: 'Also on C925' }] },
      ] })),
      http.get('/api/db/catalog/projects/7/scheduler', () => ok({ id: 7, name: 'Heart', exposure_templates: [], targets: [] })),
      http.post('/api/director/v1/projects/project/attach', async ({ request }) => { posts.push({ url: 'attach', body: await request.json() }); return ok({ into: { id: 'project', name: 'Heart', revision: 1 }, absorbed: { id: 'other', name: 'Heart by RedCat', revision: 1 }, moved_links: 1, framing_taken: true, plan_taken: false }); }),
      http.post('/api/director/v1/projects/project/detach', async ({ request }) => { posts.push({ url: 'detach', body: await request.json() }); return ok({ id: 'fresh', name: 'Heart', revision: 1 }); }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    render(<QueryClientProvider client={client}><MemoryRouter initialEntries={['/plan?plan=project']}>
      <ProjectWorkspace instanceId="instance" projectId="project" />
    </MemoryRouter></QueryClientProvider>);
    return posts;
  }

  it('offers only projects in databases this plan has none in, and attaches after a confirmation', async () => {
    const posts = mountTwo();
    fireEvent.click(await screen.findByRole('tab', { name: 'Rigs' }));
    const pick = await screen.findByLabelText('Add a rig');
    const options = Array.from((pick as HTMLSelectElement).options).map(option => option.textContent);
    expect(options).toEqual(['Choose a rig…', 'Spare data', 'Redcat data: Heart by RedCat']);
    fireEvent.change(pick, { target: { value: 'attach:other:redcat:guid-b' } });
    expect(screen.getByRole('note')).toHaveTextContent('Merge “Heart by RedCat” into this plan?');
    fireEvent.click(screen.getByRole('button', { name: 'Attach' }));
    expect(await screen.findByText('Attached Heart by RedCat: 1 database joined this plan, and its framing came along.')).toBeInTheDocument();
    expect(posts).toEqual([{ url: 'attach', body: { from_project_id: 'other' } }]);
  });

  it('says a database could not be read rather than calling its project missing', async () => {
    mountTwo();
    server.use(http.get('/api/director/v1/plans', () => ok({ warnings: [], rows: [
      { project: { id: 'project', name: 'Heart', revision: 1 }, framing: null, plan: null, activation: null,
        links: [{ catalog_slug: 'catalog', catalog_name: 'C925 data', rig: rigA, source_project_guid: 'guid-a', source_row_id: 7, source_name: 'Heart' },
          { catalog_slug: 'third', catalog_name: 'Third data', rig: { id: 'rig-c', name: 'Third', revision: 1 }, source_project_guid: 'guid-c', source_row_id: null, source_name: null, source_unread: true }] },
    ] })));
    fireEvent.click(await screen.findByRole('tab', { name: 'Rigs' }));
    const third = await screen.findByRole('group', { name: 'Third data' });
    expect(third).toHaveTextContent('database not read just now');
    expect(third).not.toHaveTextContent('project row missing');
  });

  it('detaches one database into a plan of its own, naming it after the project', async () => {
    const posts = mountTwo();
    fireEvent.click(await screen.findByRole('tab', { name: 'Rigs' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Detach Third data' }));
    expect(screen.getByRole('note')).toHaveTextContent('Make Heart in Third data a separate plan?');
    fireEvent.click(screen.getByRole('button', { name: 'Detach' }));
    expect(await screen.findByText('Detached: Heart is a separate plan.')).toBeInTheDocument();
    expect(posts).toEqual([{ url: 'detach', body: { catalog_slug: 'third', source_project_guid: 'guid-c', name: 'Heart', drop_rig_work: false } }]);
  });

  it('says what a detach would take out of the plan, and detaches only once that is confirmed', async () => {
    const posts = mountTwo();
    server.use(http.post('/api/director/v1/projects/project/detach', async ({ request }) => {
      const body = await request.json() as { drop_rig_work: boolean };
      posts.push({ url: 'detach', body });
      return body.drop_rig_work
        ? ok({ id: 'fresh', name: 'Heart', revision: 1 })
        : HttpResponse.json({ success: false, data: null, error: 'This plan has 2 exposure goals for Third data; detaching takes them out of this plan.' }, { status: 409 });
    }));
    fireEvent.click(await screen.findByRole('tab', { name: 'Rigs' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Detach Third data' }));
    fireEvent.click(screen.getByRole('button', { name: 'Detach' }));
    // "Detaching…" shows until the refusal comes back with what it would take out.
    await waitFor(() => expect(screen.getByRole('note')).toHaveTextContent('This plan has 2 exposure goals for Third data; detaching takes them out of this plan. Detach Heart anyway?'));
    fireEvent.click(screen.getByRole('button', { name: 'Detach anyway' }));
    expect(await screen.findByText('Detached: Heart is a separate plan.')).toBeInTheDocument();
    expect(posts.map(post => (post.body as { drop_rig_work: boolean }).drop_rig_work)).toEqual([false, true]);
  });

  it('adds a rig with a new project, which activation creates, and drops it again', async () => {
    mountTwo();
    fireEvent.click(await screen.findByRole('tab', { name: 'Rigs' }));
    fireEvent.change(await screen.findByLabelText('Add a rig'), { target: { value: 'new:rig-s' } });
    const spare = await screen.findByRole('group', { name: 'Spare data' });
    expect(spare).toHaveTextContent('new project on activation');
    expect(spare).toHaveTextContent('Created on activation');
    expect(screen.getByText('Plan project: rig-s')).toBeInTheDocument();
    fireEvent.click(within(spare).getByRole('button', { name: 'Drop Spare data from the plan' }));
    expect(screen.queryByRole('group', { name: 'Spare data' })).not.toBeInTheDocument();
    expect(screen.getByText('Plan project: no rigs')).toBeInTheDocument();
  });
});
