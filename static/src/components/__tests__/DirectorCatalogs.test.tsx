import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import DirectorCatalogs from '../director/DirectorCatalogs';
import type { DirectorAdoptionPlan } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const project = { id: '11111111-1111-4111-8111-111111111111', name: 'Survey', revision: 1 };
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
const guid = '33333333-3333-4333-8333-333333333333';
function fixture() {
  const state: { saved: DirectorAdoptionPlan | null; previews: DirectorAdoptionPlan[]; applies: unknown[] } = { saved: null, previews: [], applies: [] };
  const identity = () => state.saved ? { id: state.saved.catalog_id, origin_instance_id: project.id } : null;
  const report = (plan: DirectorAdoptionPlan) => ({ catalog_identity: { id: plan.catalog_id, origin_instance_id: project.id }, preview_digest: 'a'.repeat(64), applied: false,
    mappings: plan.mappings.map(mapping => ({ mapping, source_name: 'M31', project, rig })) });
  server.use(
    http.get('/api/databases', () => HttpResponse.json(ok([{ id: 'catalog', name: 'Telescope catalog' }]))),
    http.get('/api/director/v1/catalogs/catalog/discovery', () => HttpResponse.json(ok({ catalog_slug: 'catalog', catalog_name: 'Telescope catalog', catalog_identity: identity(), snapshot_digest: state.saved ? 'saved' : 'unadopted', evidence: { projects: [
      { source_row_id: 1, source_project_guid: guid, source_profile_id: 'profile-a', name: 'M31', issues: [] },
      { source_row_id: 2, source_project_guid: null, source_profile_id: 'profile-a', name: 'Broken row', issues: ['missing_project_guid'] },
    ], profiles: [{ source_profile_id: 'profile-a', project_count: 2 }] } }))),
    http.get('/api/director/v1/catalogs/catalog/mappings', () => HttpResponse.json(ok({ catalog_identity: identity(), items: state.saved?.mappings ?? [], next_after: null }))),
    http.get('/api/director/v1/projects', () => HttpResponse.json(ok({ items: [project], next_after: null }))),
    http.get('/api/director/v1/rigs', () => HttpResponse.json(ok({ items: [rig], next_after: null }))),
    http.post('/api/director/v1/catalogs/catalog/adoption/preview', async ({ request }) => { const plan = await request.json() as DirectorAdoptionPlan; state.previews.push(plan); return HttpResponse.json(ok(report(plan))); }),
    http.post('/api/director/v1/catalogs/catalog/adoption/apply', async ({ request }) => { const body = await request.json() as { plan: DirectorAdoptionPlan }; state.applies.push(body); state.saved = body.plan; return HttpResponse.json(ok({ ...report(body.plan), applied: true })); }),
  );
  return { state, report };
}
function mount(canWrite = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}><MemoryRouter initialEntries={['/director?directorView=catalogs&directorCatalog=catalog']}>{children}</MemoryRouter></AccessContext.Provider></QueryClientProvider>;
  }
  return render(<DirectorCatalogs instanceId={project.id} />, { wrapper: Wrapper });
}
async function choose() {
  const check = await screen.findByRole('checkbox', { name: 'Select M31 (1)' });
  await waitFor(() => expect(check).toBeEnabled());
  fireEvent.click(check);
  fireEvent.change(screen.getByLabelText('Global project for M31 (1)'), { target: { value: project.id } });
  fireEvent.change(screen.getByLabelText('Rig for M31 (1)'), { target: { value: rig.id } });
}

describe('Director catalog mapping', () => {
  it('requires review, blocks invalid rows, and reloads saved links', async () => {
    const { state } = fixture(); mount(); await choose();
    expect(screen.getByRole('checkbox', { name: 'Select Broken row (2)' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Apply mappings' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Preview mappings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Apply mappings' }));
    expect(await screen.findByText('Mappings saved.')).toBeInTheDocument();
    expect(await screen.findByText('Linked', { exact: true })).toBeInTheDocument();
    expect(state.applies).toHaveLength(1);
    expect(state.saved).toEqual(state.previews[0]);
    expect(screen.getByRole('checkbox', { name: 'Select M31 (1)' })).toBeDisabled();
  });

  it('keeps the exact reviewed plan after an ambiguous apply failure', async () => {
    const { state, report } = fixture();
    server.use(http.post('/api/director/v1/catalogs/catalog/adoption/apply', async ({ request }) => {
      const body = await request.json() as { plan: DirectorAdoptionPlan; preview_digest: string }; state.applies.push(body); state.saved = body.plan;
      return state.applies.length === 1 ? HttpResponse.error() : HttpResponse.json(ok({ ...report(body.plan), applied: true }));
    }));
    mount(); await choose(); fireEvent.click(screen.getByRole('button', { name: 'Preview mappings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Apply mappings' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Network Error');
    fireEvent.click(screen.getByRole('button', { name: 'Apply mappings' }));
    await screen.findByText('Mappings saved.');
    expect(state.applies).toHaveLength(2); expect(state.applies[0]).toEqual(state.applies[1]);
  });

  it('drops a stale preview without applying automatically', async () => {
    const { state } = fixture();
    server.use(http.post('/api/director/v1/catalogs/catalog/adoption/apply', () => HttpResponse.json({ error: 'Catalog changed; review again' }, { status: 409 })));
    mount(); await choose(); fireEvent.click(screen.getByRole('button', { name: 'Preview mappings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Apply mappings' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Catalog changed');
    expect(screen.queryByRole('button', { name: 'Apply mappings' })).not.toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Select M31 (1)' })).toBeChecked();
    expect(state.previews).toHaveLength(1);
  });

  it('lets readers inspect but neither create nor apply', async () => {
    fixture(); mount(false);
    expect(await screen.findByText('Read only')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Select M31 (1)' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Preview mappings' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'New rig for M31' })).not.toBeInTheDocument();
  });

  it('reuses inline creation identity on retry', async () => {
    fixture(); const requests: unknown[] = [];
    server.use(http.post('/api/director/v1/rigs', async ({ request }) => {
      const body = await request.json() as { id: string; name: string }; requests.push(body);
      return requests.length === 1 ? HttpResponse.error() : HttpResponse.json(ok({ ...body, revision: 1 }));
    }));
    mount(); await choose(); fireEvent.click(screen.getByRole('button', { name: 'New rig for M31' }));
    fireEvent.change(screen.getByLabelText('New rig name'), { target: { value: 'Redcat' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create' }));
    await screen.findByRole('alert'); fireEvent.click(screen.getByRole('button', { name: 'Create' }));
    await waitFor(() => expect(screen.queryByLabelText('New rig name')).not.toBeInTheDocument());
    expect(requests).toHaveLength(2); expect(requests[0]).toEqual(requests[1]);
  });

  it('refuses inconsistent identity pages rather than showing mixed catalog mappings', async () => {
    fixture(); server.use(http.get('/api/director/v1/catalogs/catalog/mappings', () => HttpResponse.json(ok({ catalog_identity: { id: rig.id, origin_instance_id: project.id }, items: [], next_after: null }))));
    mount(); expect(await screen.findByRole('alert')).toHaveTextContent('Catalog identity changed');
    expect(screen.queryByRole('button', { name: 'Preview mappings' })).not.toBeInTheDocument();
  });

  it('flags a source profile change instead of presenting a stale link as current', async () => {
    const { state } = fixture();
    state.saved = { catalog_id: rig.id, mappings: [{ catalog_id: rig.id, source_project_guid: guid, source_profile_id: 'profile-a', project_id: project.id, rig_id: rig.id }] };
    server.use(http.get('/api/director/v1/catalogs/catalog/discovery', () => HttpResponse.json(ok({ catalog_slug: 'catalog', catalog_name: 'Telescope catalog', catalog_identity: { id: rig.id, origin_instance_id: project.id }, snapshot_digest: 'changed', evidence: { projects: [
      { source_row_id: 1, source_project_guid: guid, source_profile_id: 'profile-b', name: 'M31', issues: [] },
    ], profiles: [] } }))));
    mount(); expect(await screen.findByText('Source profile changed')).toBeInTheDocument();
    expect(screen.getByText('0 linked / 1 source projects')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Select M31 (1)' })).toBeDisabled();
  });

  it('rejects repeating identity cursors and leaves refresh available', async () => {
    fixture(); server.use(http.get('/api/director/v1/projects', () => HttpResponse.json(ok({ items: [project], next_after: 'repeat' }))));
    mount(); expect(await screen.findByRole('alert')).toHaveTextContent('repeated page');
    expect(screen.getByRole('button', { name: 'Refresh catalog' })).toBeEnabled();
  });

  it('shares one rig choice across valid projects from the same source profile', async () => {
    const { state } = fixture();
    const second = '44444444-4444-4444-8444-444444444444';
    server.use(http.get('/api/director/v1/catalogs/catalog/discovery', () => HttpResponse.json(ok({ catalog_slug: 'catalog', catalog_name: 'Telescope catalog', catalog_identity: null, snapshot_digest: 'shared', evidence: { projects: [
      { source_row_id: 1, source_project_guid: guid, source_profile_id: 'profile-a', name: 'M31', issues: [] },
      { source_row_id: 2, source_project_guid: second, source_profile_id: 'profile-a', name: 'M42', issues: [] },
    ], profiles: [{ source_profile_id: 'profile-a', project_count: 2 }] } }))));
    mount(); await choose();
    expect(screen.getByLabelText('Rig for M42 (2)')).toHaveValue(rig.id);
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select M42 (2)' }));
    fireEvent.change(screen.getByLabelText('Global project for M42 (2)'), { target: { value: project.id } });
    fireEvent.click(screen.getByRole('button', { name: 'Preview mappings' }));
    await screen.findByRole('button', { name: 'Apply mappings' });
    expect(state.previews[0].mappings).toHaveLength(2);
    expect(state.previews[0].mappings.map(mapping => mapping.rig_id)).toEqual([rig.id, rig.id]);
  });
});
