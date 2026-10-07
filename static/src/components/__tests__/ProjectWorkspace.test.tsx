import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import ProjectWorkspace from '../director/ProjectWorkspace';

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
function summaryHandlers(saved: { framing?: boolean; framingRevision?: number; layoutRevision?: number; planRevision?: number; shoots?: boolean; activated?: { plan_revision: number; rigs?: string[] }; databaseManagement?: boolean } = {}) {
  return [
    http.get('/api/director/v1/projects/project/framing', () => ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, draft: saved.framing ? {
      project_id: 'project', revision: saved.framingRevision ?? 2, layout_revision: saved.layoutRevision, target_name: 'M31', center: { ra_degrees: 10.68, dec_degrees: 41.27 }, position_angle_degrees: 0, mosaic: { rows: 1, columns: 1, overlap_percent: 20 },
      panel_rig_id: null, panel: null, shown_rig_ids: [], survey_id: 'dss2', view_fov_degrees: 4, updated_at_ms: 1, rig_framings: [] } : null })),
    http.get('/api/director/v1/projects/project/plan', () => ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, plan: saved.planRevision === undefined ? null : {
      project_id: 'project', revision: saved.planRevision, updated_at_ms: 1, contributions: saved.shoots ? [{ id: 'c1', rig_id: 'rig-catalog', objective_id: 'o1', enabled: true, exposure_seconds: 300, panel_ids: [], template: { template_guid: 't', template_id: 1, name: 'Ha', filter_name: 'Ha', gain: null, offset: null, bin: 1, readout_mode: null } }] : [],
      objectives: [{ id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'frames', value: 40 }, priority: 1 }] } })),
    // The rigs the activation reached: by default each rig the plan shoots.
    http.get('/api/director/v1/projects/project/activation', () => ok({ activation: saved.activated ? { project_id: 'project', revision: 3, framing_revision: 2, plan_revision: saved.activated.plan_revision,
      coordinator_instance_id: 'c', applied_at_ms: 1, rigs: (saved.activated.rigs ?? (saved.shoots ? ['rig-catalog'] : [])).map(id => ({ rig_id: id, catalog_id: 'x', project_guid: 'guid', profile_id: 'p', targets: [], plans: [] })) } : null })),
    http.get('/api/director/v1/status', () => ok({ protocol_version: 1, enabled: true, instance_id: 'instance', database_management: saved.databaseManagement ?? true })),
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
    expect(due).toHaveTextContent('Not activated yet');
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
    mount(links, '/plan?plan=project', { framing: true, framingRevision: 5, layoutRevision: 2, planRevision: 4, shoots: true, activated: { plan_revision: 4 } });
    expect(await screen.findByTestId('summary-activation')).toHaveTextContent('On the rigs');
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
  });
  it('asks for one when the layout moved after the activation', async () => {
    mount(links, '/plan?plan=project', { framing: true, framingRevision: 5, layoutRevision: 5, planRevision: 4, shoots: true, activated: { plan_revision: 4 } });
    expect(await screen.findByRole('region', { name: 'Activation due' })).toHaveTextContent('Saved framing not on the rigs yet');
  });
  it('does not ask for an activation the rigs already have', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4 } });
    // Not "Active", which is a Target Scheduler project state of its own.
    expect(await screen.findByTestId('summary-activation')).toHaveTextContent('On the rigs');
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
    // The summary line still opens the activation, to push it again.
    fireEvent.click(screen.getByTestId('summary-activation'));
    expect(screen.getByRole('dialog', { name: 'Activate on the rigs' })).toBeInTheDocument();
  });
  it('asks again for a rig the last activation left out, though the revisions match', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 4, shoots: true, activated: { plan_revision: 4, rigs: [] } });
    expect(await screen.findByRole('region', { name: 'Activation due' })).toHaveTextContent('C925 data not activated yet');
    expect(screen.queryByTestId('summary-activation')).not.toBeInTheDocument();
  });
  it('leaves the activation bar out on a server that does not write rig databases', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 1, shoots: true, databaseManagement: false });
    expect(await screen.findByTestId('summary-drafts-only')).toHaveTextContent('Drafts only');
    // The summary still says where it stands and opens the preview.
    expect(await screen.findByTestId('summary-activation')).toHaveTextContent('Not activated yet');
    expect(screen.queryByRole('region', { name: 'Activation due' })).not.toBeInTheDocument();
  });
  it('sums the plan up once: target, goals, each rig and whether it is ready, and the activation', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 5, activated: { plan_revision: 4 } });
    expect(await screen.findByTestId('summary-target')).toHaveTextContent('M31');
    expect(await screen.findByTestId('summary-goals')).toHaveTextContent('H-alpha · 40 frames per rig');
    const summary = screen.getByRole('region', { name: 'Plan summary' });
    expect(await within(summary).findByText('C925 data')).toBeInTheDocument();
    expect(within(summary).getAllByText(/no rig profile/).length).toBe(2);
    // Both databases hold a project, but neither rig shoots the plan now.
    expect(within(summary).getAllByText(/· off/).length).toBe(2);
    expect(await screen.findByTestId('summary-activation')).toHaveTextContent('plan not activated');
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

  it('detaches one database into a plan of its own, naming it after the project', async () => {
    const posts = mountTwo();
    fireEvent.click(await screen.findByRole('tab', { name: 'Rigs' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Detach Third data' }));
    expect(screen.getByRole('note')).toHaveTextContent('Make Heart in Third data a separate plan?');
    fireEvent.click(screen.getByRole('button', { name: 'Detach' }));
    expect(await screen.findByText('Detached: Heart is a separate plan.')).toBeInTheDocument();
    expect(posts).toEqual([{ url: 'detach', body: { catalog_slug: 'third', source_project_guid: 'guid-c', name: 'Heart' } }]);
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
