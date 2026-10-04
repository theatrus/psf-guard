import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import type { DirectorPlanRow } from '../../api/directorTypes';
import { server } from '../../test/msw-server';
import RigTargetSelect from '../header/RigTargetSelect';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });
const project = (id: number, name: string, guid: string | null) => ({ id, guid, profile_id: 'p', profile_name: 'P', name, display_name: name, description: null, has_files: true, state: 1, latest_image_date: 1_705_352_400 });
const target = (id: number, projectId: number, name: string) => ({ id, project_id: projectId, name, active: true, has_files: true });

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}{location.search}</output>;
}

function mount(route: string, plans: DirectorPlanRow[] | null = null) {
  server.use(
    http.get('/api/databases', () => ok([{ id: 'c925', name: 'C925', path: '/a' }, { id: 'rc51', name: 'RC51', path: '/b' }])),
    http.get('/api/db/c925/projects', () => ok([project(1, 'M31', 'GUID-1'), project(2, 'Pelican', null), project(3, 'Veil', null)])),
    http.get('/api/db/rc51/projects', () => ok([project(7, 'M31', 'guid-1')])),
    http.get('/api/db/c925/targets', () => ok([target(10, 1, 'M31 panel 1'), target(11, 1, 'M31 panel 2'), target(20, 2, 'Pelican'), target(30, 3, 'Veil east'), target(31, 3, 'Veil west')])),
    http.get('/api/db/rc51/targets', () => ok([target(70, 7, 'M31')])),
    http.get('/api/director/v1/status', () => HttpResponse.json({ success: true, data: plans ? { protocol_version: 1, enabled: true, instance_id: '11111111-1111-4111-8111-111111111111', database_management: true } : { protocol_version: 1, enabled: false, instance_id: null, database_management: true }, error: null })),
    http.get('/api/director/v1/plans', () => HttpResponse.json({ success: true, data: { rows: plans ?? [], warnings: [] }, error: null })),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></QueryClientProvider>;
  return render(<RigTargetSelect />, { wrapper: Wrapper });
}

describe('rig and target switcher', () => {
  it('lists every rig of a plan shot by several, each with its targets, and moves the review scope', async () => {
    mount('/grid?db=c925&project=1');
    const select = await screen.findByRole('combobox', { name: 'Rig and target' });
    const groups = await within(select).findAllByRole('group');
    expect(groups.map(group => group.getAttribute('label'))).toEqual(['C925', 'RC51']);
    expect(within(groups[0]).getAllByRole('option').map(option => option.textContent)).toEqual(['C925 · all targets', 'M31 panel 1', 'M31 panel 2']);
    fireEvent.change(select, { target: { value: 'rc51:7:70' } });
    expect(screen.getByTestId('location')).toHaveTextContent('/grid?db=rc51&project=7&target=70');
  });

  it("lists a lone rig's targets, and only names the database when there is nothing to choose", async () => {
    const lone = mount('/grid?db=c925&project=3');
    const select = await screen.findByRole('combobox', { name: 'Rig and target' });
    expect(within(select).getAllByRole('option').map(option => option.textContent)).toEqual(['All targets', 'Veil east', 'Veil west']);
    fireEvent.change(select, { target: { value: 'c925:3:31' } });
    expect(screen.getByTestId('location')).toHaveTextContent('/grid?db=c925&project=3&target=31');
    lone.unmount();
    mount('/grid?db=c925&project=2');
    const fixed = await screen.findByLabelText('Rig and target: C925 · Pelican');
    expect(fixed).toHaveClass('rig-target-fixed');
    expect(fixed).toHaveTextContent('Rig · target');
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  });

  it('says so when no project is in scope', async () => {
    mount('/');
    expect(await screen.findByLabelText('Rig: none in scope')).toBeInTheDocument();
  });

  it('opens Images when a rig is chosen from the Library, and reads a stale target as all targets', async () => {
    mount('/?db=c925&project=1&target=999&dbfilter=c925');
    const select = await screen.findByRole('combobox', { name: 'Rig and target' });
    expect(select).toHaveValue('c925:1:');
    fireEvent.change(select, { target: { value: 'rc51:7:' } });
    expect(screen.getByTestId('location')).toHaveTextContent('/grid?db=rc51&project=7&dbfilter=c925');
  });

  it("takes a workspace's rigs from its plan and waits for a choice when the review database is not one", async () => {
    const rig = { id: 'r', name: 'r', revision: 1 };
    const link = (slug: string, name: string, row: number | null) => ({ catalog_slug: slug, catalog_name: name, rig, source_project_guid: 'guid-1', source_row_id: row, source_name: 'M31', source_state: 1, earliest_capture_s: null, latest_capture_s: null, targets: [] });
    const plan: DirectorPlanRow = { project: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'M31', revision: 1 }, links: [link('c925', 'C925', 1), link('rc51', 'RC51', 7), link('gone', 'Gone', null)], progress: null, framing: null, plan: null, activation: null };
    mount('/plan?plan=guid-1&db=elsewhere&project=5', [plan]);
    const select = await screen.findByRole('combobox', { name: 'Rig and target' });
    const groups = await within(select).findAllByRole('group');
    // The link with no project row is not a rig you can review.
    expect(groups.map(group => group.getAttribute('label'))).toEqual(['C925', 'RC51']);
    expect(within(select).getByRole('option', { name: 'Choose a rig' })).toBeDisabled();
    fireEvent.change(select, { target: { value: 'rc51:7:' } });
    // In a workspace the choice moves the scope in place, for Images and the open editor.
    expect(screen.getByTestId('location')).toHaveTextContent('/plan?plan=guid-1&db=rc51&project=7');
  });
});
