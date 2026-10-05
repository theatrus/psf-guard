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
// The editor draws one block per rig; the stub draws what the workspace
// gives each linked rig, and the list's footer, and holds one draft field so
// the save bar and the tabs have an edit to track.
vi.mock('../director/PlanEditor', async () => {
  const { useState } = await vi.importActual<typeof import('react')>('react');
  const { useDraftSection } = await vi.importActual<typeof import('../director/pageDraftsState')>('../director/pageDraftsState');
  function Goal() {
    const [goal, setGoal] = useState('40');
    useDraftSection('plan', { label: 'Plan', order: 2, unsaved: goal !== '40', save: async () => true, discard: () => setGoal('40') });
    return <input aria-label="Stub goal" value={goal} onChange={event => setGoal(event.target.value)} />;
  }
  return { default: ({ projectId, linkedRigIds = [], rigExtras, footer }: {
    projectId: string;
    linkedRigIds?: string[];
    rigExtras?: (rig: { rig: { id: string; name: string; revision: number }; catalog_slug: string; catalog_name: string }) => { place?: React.ReactNode; below?: React.ReactNode };
    footer?: React.ReactNode;
  }) => <div>
    <output>{`Plan ${projectId}`}</output>
    {linkedRigIds.map(id => {
      const extras = rigExtras?.({ rig: { id, name: id, revision: 1 }, catalog_slug: id, catalog_name: id }) ?? {};
      return <div key={id}>{extras.place}{extras.below}</div>;
    })}
    <Goal />
    {footer}
  </div> };
});
vi.mock('../director/ActivationPanel', () => ({ default: ({ projectId }: { projectId: string }) => <output>{`Activation ${projectId}`}</output> }));
vi.mock('../director/ObservingPreferences', () => ({ default: () => <output>Observing preferences</output> }));
const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
function Where() { return <output data-testid="where">{useLocation().search}</output>; }
/** The saved framing, plan and last activation the summary reads. */
function summaryHandlers(saved: { framing?: boolean; planRevision?: number; activated?: { plan_revision: number } } = {}) {
  return [
    http.get('/api/director/v1/projects/project/framing', () => ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, draft: saved.framing ? {
      project_id: 'project', revision: 2, target_name: 'M31', center: { ra_degrees: 10.68, dec_degrees: 41.27 }, position_angle_degrees: 0, mosaic: { rows: 1, columns: 1, overlap_percent: 20 },
      panel_rig_id: null, panel: null, shown_rig_ids: [], survey_id: 'dss2', view_fov_degrees: 4, updated_at_ms: 1, rig_framings: [] } : null })),
    http.get('/api/director/v1/projects/project/plan', () => ok({ project: { id: 'project', name: 'Andromeda', revision: 1 }, plan: saved.planRevision === undefined ? null : {
      project_id: 'project', revision: saved.planRevision, updated_at_ms: 1, contributions: [],
      objectives: [{ id: 'o1', bandpass_id: 'h_alpha', purpose: 'faint_detail', goal: { kind: 'frames', value: 40 }, priority: 1 }] } })),
    http.get('/api/director/v1/projects/project/activation', () => ok({ activation: saved.activated ? { project_id: 'project', revision: 3, framing_revision: 2, plan_revision: saved.activated.plan_revision,
      coordinator_instance_id: 'c', applied_at_ms: 1, rigs: [] } : null })),
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
  it('seeds framing from the first linked database and keeps each database editor in its own tab', async () => {
    mount(links);
    expect(await screen.findByRole('heading', { name: 'Andromeda' })).toBeInTheDocument();
    expect(await screen.findByText('Framing project:M31@7.5')).toBeInTheDocument();
    expect(screen.getByText('Plan project')).toBeInTheDocument();
    expect(screen.getByText('Activation project')).toBeInTheDocument();
    expect(screen.getAllByText('Project row missing in this database').length).toBeGreaterThan(0);
    // The editors load once their tab is opened, the arrival database first.
    expect(screen.queryByText('Source editor catalog:7')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: 'Rig databases' }));
    expect(screen.getByText('Source editor catalog:7')).toBeInTheDocument();
    expect(screen.getByText(/opened from here/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Target Scheduler rows/ }));
    expect(screen.queryByText('Source editor catalog:7')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Library' })).toHaveAttribute('href', '/?db=catalog');
  });
  it('keeps the tab in the address and opens the one a link names', async () => {
    mount(links, '/plan?plan=project&planTab=priority');
    expect(await screen.findByRole('tab', { name: 'Priority and defaults' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText('Observing preferences').closest('[role="tabpanel"]')).not.toHaveAttribute('hidden');
    expect(screen.getByText('Plan project').closest('[role="tabpanel"]')).toHaveAttribute('hidden');
    fireEvent.click(screen.getByRole('tab', { name: 'Plan' }));
    expect(screen.getByRole('tab', { name: 'Plan' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('where')).toHaveTextContent('planTab=plan');
    expect(screen.getByTestId('where')).toHaveTextContent('plan=project');
    // Arrow keys move along the tabs.
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Plan' }), { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: 'Activate' })).toHaveAttribute('aria-selected', 'true');
  });
  it('keeps an unsaved edit across tabs and marks the tab that holds it', async () => {
    mount(links, '/plan?plan=project&planTab=plan');
    const goal = await screen.findByLabelText('Stub goal');
    fireEvent.change(goal, { target: { value: '120' } });
    expect(screen.getByRole('tab', { name: /Plan/ })).toContainElement(screen.getByLabelText('edited'));
    expect(screen.getByRole('region', { name: 'Unsaved changes' })).toHaveTextContent('Unsaved changes in Plan.');
    fireEvent.click(screen.getByRole('tab', { name: 'Framing' }));
    fireEvent.click(screen.getByRole('tab', { name: /Plan/ }));
    expect(screen.getByLabelText('Stub goal')).toHaveValue('120');
  });
  it('sums the plan up once: target, goals, each rig and whether it is ready, and the activation', async () => {
    mount(links, '/plan?plan=project', { framing: true, planRevision: 5, activated: { plan_revision: 4 } });
    expect(await screen.findByTestId('summary-target')).toHaveTextContent('M31');
    expect(await screen.findByTestId('summary-goals')).toHaveTextContent('H-alpha · 40 frames per rig');
    const summary = screen.getByRole('region', { name: 'Plan summary' });
    expect(await within(summary).findByText('C925 data')).toBeInTheDocument();
    expect(within(summary).getAllByText(/no rig profile/).length).toBe(2);
    expect(await screen.findByTestId('summary-activation')).toHaveTextContent('saved plan not sent yet');
  });
  it('opens on framing when no framing is saved yet', async () => {
    mount(links, '/plan?plan=project', { framing: false });
    expect(await screen.findByRole('tab', { name: 'Framing' })).toHaveAttribute('aria-selected', 'true');
  });
  it('frames an unlinked project without a seed', async () => {
    mount([]);
    expect(await screen.findByText('Framing project:no seed')).toBeInTheDocument();
    expect(screen.getByText(/No database holds this project yet/)).toBeInTheDocument();
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
    const pick = await screen.findByLabelText('Attach a project from another database');
    const options = Array.from((pick as HTMLSelectElement).options).map(option => option.textContent);
    expect(options).toEqual(['Choose a project…', 'Redcat data: Heart by RedCat']);
    fireEvent.change(pick, { target: { value: 'other:redcat:guid-b' } });
    expect(screen.getByRole('note')).toHaveTextContent('“Heart by RedCat” joins this plan and is retired');
    fireEvent.click(screen.getByRole('button', { name: 'Attach' }));
    expect(await screen.findByText('Attached Heart by RedCat: 1 database joined this plan, and its framing came along.')).toBeInTheDocument();
    expect(posts).toEqual([{ url: 'attach', body: { from_project_id: 'other' } }]);
  });

  it('detaches one database into a plan of its own, naming it after the project', async () => {
    const posts = mountTwo();
    fireEvent.click(await screen.findByRole('tab', { name: 'Rig databases' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Detach Third data' }));
    expect(screen.getByRole('note')).toHaveTextContent('Heart in Third data becomes a plan of its own');
    fireEvent.click(screen.getByRole('button', { name: 'Detach' }));
    expect(await screen.findByText('Detached: Heart is a plan of its own again.')).toBeInTheDocument();
    expect(posts).toEqual([{ url: 'detach', body: { catalog_slug: 'third', source_project_guid: 'guid-c', name: 'Heart' } }]);
  });
});
