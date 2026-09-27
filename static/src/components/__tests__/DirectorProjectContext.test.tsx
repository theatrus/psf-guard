import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { OPEN_SETTINGS_EVENT } from '../../utils/settingsIntent';
import DirectorProjectContext from '../director/DirectorProjectContext';

vi.mock('../ProjectSchedulerDialog', () => ({
  ProjectPlanEditor: ({ dbId, projectId }: { dbId: string; projectId: number }) => <output>{`Source editor ${dbId}:${projectId}`}</output>,
}));
vi.mock('../director/ActivationPanel', () => ({
  default: ({ projectId }: { projectId: string }) => <output>{`Activation ${projectId}`}</output>,
}));
vi.mock('../director/PlanEditor', () => ({
  default: ({ projectId }: { projectId: string }) => <output>{`Plan ${projectId}`}</output>,
}));
vi.mock('../director/FramingView', () => ({
  default: ({ projectId, seed }: { projectId: string; seed: { name: string; center: { ra_degrees: number } } | null }) => <output>{`Framing ${projectId}:${seed ? `${seed.name}@${seed.center.ra_degrees}` : 'no seed'}`}</output>,
}));
const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });
function mount(profile = 'profile', row = 7, linked = true) {
  server.use(
    http.get('/api/director/v1/catalogs/catalog/discovery', () => ok({
      catalog_slug: 'catalog', catalog_name: 'C925 data', catalog_identity: null, snapshot_digest: 'snapshot',
      evidence: { projects: [{ source_row_id: row, source_project_guid: 'guid', source_profile_id: profile, name: 'Andromeda subs', issues: [] }], profiles: [] },
    })),
    http.get('/api/director/v1/catalogs/catalog/mappings', () => ok({ catalog_identity: null, rig: { id: 'rig', name: 'C925', revision: 1 }, items: linked ? [{
      source_project_guid: 'guid', source_profile_id: 'profile', project_id: 'project', rig_id: 'rig',
    }] : [], next_after: null })),
    http.get('/api/director/v1/projects', () => ok({ items: [{ id: 'project', name: 'Andromeda', revision: 1 }], next_after: null })),
    http.get('/api/director/v1/rigs', () => ok({ items: [{ id: 'rig', name: 'C925', revision: 1 }], next_after: null })),
    http.get('/api/db/catalog/projects/7/scheduler', () => ok({ id: 7, name: 'Andromeda subs', exposure_templates: [], targets: [{ id: 1, name: 'M31', active: true, ra_hours: 0.5, dec_degrees: 41.27, epoch_code: 2, rotation: 35, roi: 100, exposure_plans: [] }] })),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><MemoryRouter initialEntries={['/director?db=catalog&project=7&dbfilter=catalog&directorSource=catalog&directorView=projects']}>
    <DirectorProjectContext instanceId="instance" slug="catalog" projectId={7} />
  </MemoryRouter></QueryClientProvider>);
}

describe('existing project planning context', () => {
  it('uses explicit source links and the existing project editor, retaining Overview return scope', async () => {
    mount();
    expect(await screen.findByRole('heading', { name: 'Andromeda' })).toBeInTheDocument();
    expect(screen.getByText('Database-backed', { exact: true })).toBeInTheDocument();
    expect(screen.getByText('Source editor catalog:7')).toBeInTheDocument();
    expect(await screen.findByText('Framing project:M31@7.5')).toBeInTheDocument();
    expect(screen.getByText('Plan project')).toBeInTheDocument();
    expect(screen.getByText('Activation project')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Overview' })).toHaveAttribute('href', '/?db=catalog&project=7&dbfilter=catalog');
    expect(screen.queryByRole('button', { name: 'New project' })).not.toBeInTheDocument();
  });
  it('does not apply an old rig link after a source profile change', async () => {
    mount('changed');
    await screen.findByRole('heading', { name: 'Andromeda subs' });
    expect(screen.queryByText('C925', { exact: true })).not.toBeInTheDocument();
    expect(screen.getAllByText('Not linked')).toHaveLength(1);
  });
  it('shows an unlinked existing project without creating metadata', async () => {
    mount('profile', 7, false);
    await screen.findByRole('heading', { name: 'Andromeda subs' });
    expect(screen.getByText('Source editor catalog:7')).toBeInTheDocument();
    expect(screen.getByText(/Link this project under Project planning links/)).toBeInTheDocument();
    expect(screen.queryByText(/^Framing project/)).not.toBeInTheDocument();
  });
  it('never falls back to a different source row', async () => {
    mount('profile', 8);
    expect(await screen.findByRole('alert')).toHaveTextContent('Project no longer exists');
    expect(screen.queryByText('Source editor catalog:7')).not.toBeInTheDocument();
  });
  it('opens linking under the exact database settings', async () => {
    mount();
    await screen.findByRole('heading', { name: 'Andromeda' });
    const listener = vi.fn();
    window.addEventListener(OPEN_SETTINGS_EVENT, listener);
    try {
      fireEvent.click(screen.getByRole('button', { name: 'Project planning links' }));
      expect((listener.mock.calls[0][0] as CustomEvent).detail).toEqual({ intent: { kind: 'director-links', dbId: 'catalog' } });
    } finally { window.removeEventListener(OPEN_SETTINGS_EVENT, listener); }
  });
});
