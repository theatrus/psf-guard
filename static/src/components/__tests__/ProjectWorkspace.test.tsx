import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
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
vi.mock('../director/PlanEditor', () => ({ default: ({ projectId }: { projectId: string }) => <output>{`Plan ${projectId}`}</output> }));
vi.mock('../director/ActivationPanel', () => ({ default: ({ projectId }: { projectId: string }) => <output>{`Activation ${projectId}`}</output> }));
const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
const rig = { id: 'rig', name: 'C925', revision: 1 };

function mount(links: Array<{ catalog_slug: string; catalog_name: string; source_row_id: number | null; source_name: string | null }>, route = '/plan?db=catalog&plan=project') {
  server.use(
    http.get('/api/director/v1/plans', () => ok({ warnings: [], rows: [{ project: { id: 'project', name: 'Andromeda', revision: 1 }, framing: null, plan: null, activation: null,
      links: links.map(link => ({ ...link, rig, source_project_guid: 'guid' })) }] })),
    http.get('/api/db/catalog/projects/7/scheduler', () => ok({ id: 7, name: 'Andromeda subs', exposure_templates: [], targets: [{ id: 1, name: 'M31', active: true, ra_hours: 0.5, dec_degrees: 41.27, epoch_code: 2, rotation: 35, roi: 100, exposure_plans: [] }] })),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>
    <ProjectWorkspace instanceId="instance" projectId="project" />
  </MemoryRouter></QueryClientProvider>);
}

describe('project workspace', () => {
  const links = [{ catalog_slug: 'catalog', catalog_name: 'C925 data', source_row_id: 7, source_name: 'Andromeda subs' }, { catalog_slug: 'redcat', catalog_name: 'Redcat data', source_row_id: null, source_name: null }];
  it('seeds framing from the first linked database and opens the editor of the database it came from', async () => {
    mount(links);
    expect(await screen.findByRole('heading', { name: 'Andromeda' })).toBeInTheDocument();
    expect(await screen.findByText('Framing project:M31@7.5')).toBeInTheDocument();
    expect(screen.getByText('Plan project')).toBeInTheDocument();
    expect(screen.getByText('Activation project')).toBeInTheDocument();
    expect(screen.getByText('Project row missing in this database')).toBeInTheDocument();
    // The Library's Planning button names its database in `db`; that editor is open on arrival.
    expect(screen.getByText('Source editor catalog:7')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Targets and exposures/ }));
    expect(screen.queryByText('Source editor catalog:7')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Library' })).toHaveAttribute('href', '/?db=catalog');
  });
  it('keeps every database editor closed when it did not arrive from one', async () => {
    mount(links, '/plan?db=elsewhere&plan=project');
    expect(await screen.findByRole('heading', { name: 'Andromeda' })).toBeInTheDocument();
    expect(screen.queryByText('Source editor catalog:7')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Targets and exposures/ }));
    expect(screen.getByText('Source editor catalog:7')).toBeInTheDocument();
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
    expect(screen.getByRole('note')).toHaveTextContent('“Heart by RedCat” is retired as a plan and its database joins this one');
    fireEvent.click(screen.getByRole('button', { name: 'Attach' }));
    expect(await screen.findByText('Attached Heart by RedCat: 1 database joined this plan, and its framing came along.')).toBeInTheDocument();
    expect(posts).toEqual([{ url: 'attach', body: { from_project_id: 'other' } }]);
  });

  it('detaches one database into a plan of its own, naming it after the project', async () => {
    const posts = mountTwo();
    fireEvent.click(await screen.findByRole('button', { name: 'Detach Third data' }));
    expect(screen.getByRole('note')).toHaveTextContent('Heart in Third data becomes a plan of its own');
    fireEvent.click(screen.getByRole('button', { name: 'Detach' }));
    expect(await screen.findByText('Detached: Heart is a plan of its own again.')).toBeInTheDocument();
    expect(posts).toEqual([{ url: 'detach', body: { catalog_slug: 'third', source_project_guid: 'guid-c', name: 'Heart' } }]);
  });
});
