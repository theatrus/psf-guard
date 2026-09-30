import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import Overview from '../Overview';
import { setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import type { DirectorPlanLink, DirectorPlanRow } from '../../api/directorTypes';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });
const now = Math.floor(Date.now() / 1000);
const shared = 'AAAAAAAA-0000-4000-8000-000000000001';
const instance = '11111111-1111-4111-8111-111111111111';
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'RedCat', revision: 1 };

function project(id: number, name: string, guid: string | null, state: number, accepted: number, desired: number) {
  return {
    id, guid, profile_id: 'profile', profile_name: 'Profile', name, display_name: name, has_files: true, state, target_count: 1,
    total_images: accepted + 2, accepted_images: accepted, rejected_images: 1, pending_images: 1, total_desired: desired, files_found: accepted + 2, files_missing: 0,
    date_range: { earliest: now - 86_400 * 3, latest: now - 3_600 * 5 }, filters_used: ['Ha'], recent_images: [],
  };
}

const catalogs: Record<string, ReturnType<typeof project>[]> = {
  redcat: [project(1, 'Heart Nebula', shared, 1, 10, 40), project(2, 'Pelican', null, 1, 20, 20), project(3, 'Old field', null, 3, 4, 10)],
  c925: [project(1, 'Heart Nebula', shared, 2, 5, 40), project(4, 'Draft field', null, 0, 0, 10)],
};

const link = (slug: string, name: string, row: number, guid = `guid-${slug}-${row}`): DirectorPlanLink => ({ catalog_slug: slug, catalog_name: name, rig, source_project_guid: guid, source_row_id: row, source_name: null, source_state: 1, earliest_capture_s: null, latest_capture_s: null, targets: [] });
const plan = (id: string, name: string, links: DirectorPlanLink[], extra: Partial<DirectorPlanRow> = {}): DirectorPlanRow => ({ project: { id, name, revision: 1 }, links, progress: null, framing: null, plan: null, activation: null, ...extra });
const plans = [
  plan('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'Heart Nebula', [link('redcat', 'RedCat', 1, shared), link('c925', 'C925', 1, shared)], { plan: { revision: 1, objectives: 2, rigs: 2 } }),
  plan('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', 'Pelican', [link('redcat', 'RedCat', 2)], { activation: { revision: 1, applied_at_ms: 1_700_000_000_000, rigs: 1 } as DirectorPlanRow['activation'] }),
  plan('cccccccc-cccc-4ccc-8ccc-cccccccccccc', 'Bare', []),
];

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}{location.search}</output>;
}

function mount(route: string, puts: { path: string; body: unknown }[] = []) {
  server.use(
    http.get('/api/info', () => ok({ version: '0.11.0', allow_database_management: true, banner: null })),
    http.get('/api/databases', () => ok([{ id: 'redcat', name: 'RedCat', path: '/a.sqlite' }, { id: 'c925', name: 'C925', path: '/b.sqlite' }])),
    http.get('/api/db/:dbId/projects/overview', ({ params }) => ok(catalogs[String(params.dbId)])),
    http.get('/api/db/:dbId/targets/overview', () => ok([])),
    http.get('/api/db/:dbId/stats/overall', () => ok({ total_projects: 3, active_projects: 2, total_targets: 3, active_targets: 3, total_images: 40, accepted_images: 26, rejected_images: 4, pending_images: 6, total_desired: 80, files_found: 40, files_missing: 0, unique_filters: ['Ha'], date_range: { earliest: now - 86_400 * 3, latest: now - 3_600 * 5 }, recent_activity: [] })),
    http.get('/api/settings/export', () => ok({ default_layout: 'flat' })),
    http.get('/api/director/v1/status', () => ok({ protocol_version: 1, enabled: true, instance_id: instance, acquisition_available: false, database_management: true })),
    http.get('/api/director/v1/plans', () => ok({ rows: plans, warnings: [] })),
    http.put('/api/db/:dbId/projects/:id', async ({ request, params }) => { puts.push({ path: `${params.dbId}/${params.id}`, body: await request.json() }); return ok({ updated: true }); }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></QueryClientProvider>;
  return render(<Overview />, { wrapper: Wrapper });
}

const compact = { showNightChip: true, showAllChip: true, advanceOnGrade: true, projectPickerGrouping: 'activity' as const, libraryDensity: 'compact' as const };
const names = () => screen.getAllByTestId('library-row').map(row => within(row).getByRole('button', { name: /image grid/ }).textContent);

describe('the Library with Planning', () => {
  afterEach(() => setDisplayPreferences(compact));

  it('carries each plan: its stage and workspace on a family, the workspace on a lone row, and plans without a database', async () => {
    setDisplayPreferences(compact);
    mount('/');
    const family = await screen.findByTestId('library-family');
    expect(await within(family).findByText('Planned')).toBeInTheDocument();
    // Members of a family share the outer pill's way to the plan.
    expect(within(family).queryByRole('button', { name: /in Planning$/ })).not.toBeInTheDocument();
    fireEvent.click(within(family).getByRole('button', { name: 'Open the Heart Nebula plan' }));
    // The plan's address is the Target Scheduler GUID its rigs share.
    // The member it was opened from becomes the scope, so the workspace opens that database.
    expect(screen.getByTestId('location')).toHaveTextContent(new RegExp(`^/plan\\?db=(redcat|c925)&project=1&plan=${shared.toLowerCase()}$`));
  });

  it('opens a lone project straight into its workspace and lists the plan nothing shoots', async () => {
    setDisplayPreferences(compact);
    mount('/');
    fireEvent.click(await screen.findByRole('button', { name: 'Open Pelican in Planning' }));
    expect(screen.getByTestId('location')).toHaveTextContent('/plan?db=redcat&project=2&plan=guid-redcat-2');
  });

  it('narrows by Show and search in the URL, family by family', async () => {
    setDisplayPreferences(compact);
    mount('/');
    const showSelect = await screen.findByRole('combobox', { name: 'Show projects' });
    await screen.findByTestId('library-family');
    expect(within(showSelect).getByRole('option', { name: 'No database' })).toBeInTheDocument();
    // Inactive keeps the Heart family whole: one of its rigs is inactive.
    fireEvent.change(showSelect, { target: { value: 'inactive' } });
    expect(screen.getByTestId('location')).toHaveTextContent('/?show=inactive');
    expect(screen.getByText('Showing 2 of 5 projects.')).toBeInTheDocument();
    expect(names().sort()).toEqual(['Heart Nebula', 'Heart Nebula']);
    fireEvent.change(showSelect, { target: { value: 'done' } });
    expect(names()).toEqual(['Pelican']);
    fireEvent.change(showSelect, { target: { value: 'draft' } });
    expect(names()).toEqual(['Draft field']);
    // Closed projects live in the archive; asking for them opens it.
    fireEvent.change(showSelect, { target: { value: 'closed' } });
    expect(await screen.findByText('Old field')).toBeInTheDocument();
    // No database: the projects go, the plan nothing shoots stays.
    fireEvent.change(showSelect, { target: { value: 'unlinked' } });
    expect(screen.queryAllByTestId('library-row')).toHaveLength(0);
    expect(await screen.findByRole('link', { name: 'Open Bare' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Show all' }));
    expect(screen.getByTestId('location')).toHaveTextContent(/^\/$/);
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search projects or targets' }), { target: { value: 'peli' } });
    expect(screen.getByTestId('location')).toHaveTextContent('/?q=peli');
    await waitFor(() => expect(names()).toEqual(['Pelican']));
  });

  it("changes a project's state in its database from the row", async () => {
    setDisplayPreferences(compact);
    const puts: { path: string; body: unknown }[] = [];
    mount('/', puts);
    const select = await screen.findByRole('combobox', { name: 'State of Pelican in RedCat' });
    fireEvent.change(select, { target: { value: '2' } });
    // The write in flight shows its choice and takes no second one.
    expect(select).toBeDisabled();
    expect(select).toHaveValue('2');
    fireEvent.change(select, { target: { value: '3' } });
    expect(await screen.findByText('Pelican is now Inactive on RedCat.')).toBeInTheDocument();
    expect(puts).toEqual([{ path: 'redcat/2', body: { state: 2 } }]);
    await waitFor(() => expect(screen.getByRole('combobox', { name: 'State of Pelican in RedCat' })).not.toBeDisabled());
  });
});
