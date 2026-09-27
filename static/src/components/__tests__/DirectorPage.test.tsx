import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import { useScopedDbId } from '../../hooks/useUrlState';
import DirectorPage from '../director/DirectorPage';
import type { DirectorPlanRow } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const record = { id: '11111111-1111-4111-8111-111111111111', name: 'M31', revision: 1 };
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
const enabled = { protocol_version: 1, enabled: true, instance_id: record.id, acquisition_available: false };
const row = (project = record, extra: Partial<DirectorPlanRow> = {}): DirectorPlanRow => ({ project, links: [], framing: null, plan: null, activation: null, ...extra });
function Location() {
  const location = useLocation();
  const db = useScopedDbId();
  return <output data-testid="location">{location.search}|scope={db ?? 'global'}</output>;
}
/** A test's own handlers go first: msw matches the first handler it was given. */
function mount(canWrite = true, route = '/director?db=old-catalog&project=123', handlers: Parameters<typeof server.use> = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></AccessContext.Provider></QueryClientProvider>;
  }
  server.use(
    ...handlers,
    http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))),
    http.get('/api/databases', () => HttpResponse.json(ok([{ id: 'c925', name: 'C925' }]))),
    http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([]))),
    http.get('/api/director/v1/rigs/status', () => HttpResponse.json(ok([]))),
  );
  render(<DirectorPage />, { wrapper: Wrapper });
}

describe('Director management', () => {
  it('creates with the same UUID after an ambiguous failure, and trims names', async () => {
    const requests: { id: string; name: string }[] = [];
    let committed = false;
    server.use(
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok(committed ? [row({ ...record, ...requests[0] })] : []))),
      http.post('/api/director/v1/projects', async ({ request }) => {
        requests.push(await request.json() as { id: string; name: string });
        committed = true;
        return requests.length === 1 ? HttpResponse.error() : HttpResponse.json(ok({ ...requests[0], revision: 1 }));
      }),
    );
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'New project' }));
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: '  Sky survey  ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Network Error');
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Created Sky survey.')).toBeInTheDocument();
    expect(requests).toHaveLength(2);
    expect(requests[0]).toEqual(requests[1]);
    expect(requests[0].name).toBe('Sky survey');
    expect(requests[0].id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  });

  it('keeps a stale rename draft until canceled and reloads the current revision', async () => {
    let current = record;
    const updates: unknown[] = [];
    server.use(
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok([row(current)]))),
      http.patch('/api/director/v1/projects/:id', async ({ request, params }) => {
        expect(params.id).toBe(record.id);
        const body = await request.json() as { expected_revision: number; name: string };
        updates.push(body);
        if (body.expected_revision !== current.revision) return HttpResponse.json({ error: 'Revision conflict; reload before retrying' }, { status: 409 });
        current = { ...current, name: body.name, revision: current.revision + 1 };
        return HttpResponse.json(ok(current));
      }),
    );
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Rename M31' }));
    current = { ...record, name: 'Changed elsewhere', revision: 2 };
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'Andromeda' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Revision conflict');
    expect(screen.getByLabelText('Project name')).toHaveValue('Andromeda');
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh records' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Rename Changed elsewhere' }));
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'Andromeda' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Renamed Andromeda.')).toBeInTheDocument();
    expect(updates).toEqual([{ name: 'Andromeda', expected_revision: 1 }, { name: 'Andromeda', expected_revision: 2 }]);
  });

  it('shows each plan with its stage and opens Rig planning at its linked database row', async () => {
    mount(false, undefined, [
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok([
        row(record, { links: [{ catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'g', source_row_id: 7, source_name: 'Andromeda subs' }],
          framing: { revision: 2, target_name: 'M31', panels: 4, panel_rig_id: rig.id }, plan: { revision: 1, objectives: 2, rigs: 1 }, activation: null }),
        row({ ...record, id: '33333333-3333-4333-8333-333333333333', name: 'Bare' }),
      ]))),
      http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([{ rig, catalog_slug: 'c925', catalog_name: 'C925', profile: null,
        field_of_view: { width_degrees: 0.7, height_degrees: 0.5, pixel_scale_arcsec: 0.41, focal_ratio: 10 }, default_exposure_seconds: { broadband: 120, narrowband: 300 } }]))),
      http.get('/api/director/v1/rigs/status', () => HttpResponse.json(ok([{ rig, checkins: [], status: { rig_id: rig.id, session_id: 's1', reported_at_ms: 1_700_000_000_000, received_at_ms: 1_700_000_000_001, payload: { phase: 'exposing' } } }]))),
    ]);
    expect(await screen.findByText('M31')).toBeInTheDocument();
    expect(screen.getByText('Planned: 2 objectives, 1 rig, not activated')).toBeInTheDocument();
    expect(screen.getByText('C925: Andromeda subs')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Rig planning' })).toHaveAttribute('href', '/director?db=c925&project=7&dbfilter=c925&directorSource=c925&directorView=projects');
    expect(screen.getByText('Not linked to any database')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Link a database' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /New project|Rename/ })).not.toBeInTheDocument();
    expect(await screen.findByText(/Field 42.0′ × 30.0′, 0.41″\/px, camera not reported yet/)).toBeInTheDocument();
    expect(screen.getByText(/Plugin: exposing/)).toBeInTheDocument();
    expect(screen.getByTestId('location')).toHaveTextContent('?db=old-catalog&project=123|scope=global');
  });

  it('folds the old tab links into the one page while keeping catalog URL state', async () => {
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok([]))));
    mount(true, '/director?db=old-catalog&project=123&directorView=sites');
    expect(await screen.findByText('C925')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Plans' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'New rig' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Sites' })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('?db=old-catalog&project=123&directorView=projects|scope=global'));
  });

  it.each([{ ...enabled, enabled: false }, { ...enabled, protocol_version: 2 }])('does not load records when unavailable: %j', async status => {
    const list = vi.fn(() => HttpResponse.json(ok([])));
    mount();
    server.use(http.get('/api/director/v1/status', () => HttpResponse.json(ok(status))), http.get('/api/director/v1/plans', list));
    expect(await screen.findByText('Director management is unavailable on this server.')).toBeInTheDocument();
    expect(list).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'New project' })).not.toBeInTheDocument();
  });

  it('rejects overlong multibyte names before sending them', async () => {
    const create = vi.fn(() => HttpResponse.json(ok(record)));
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok([]))), http.post('/api/director/v1/projects', create));
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'New project' }));
    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'é'.repeat(300) } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('512 UTF-8 bytes');
    expect(create).not.toHaveBeenCalled();
  });

  it('recovers a failed initial listing without leaving a false empty state', async () => {
    let fail = true;
    server.use(http.get('/api/director/v1/plans', () => fail
      ? HttpResponse.json({ error: 'Director metadata is busy; retry shortly' }, { status: 503 })
      : HttpResponse.json(ok([row()]))));
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent('busy');
    expect(screen.queryByText('No projects yet.')).not.toBeInTheDocument();
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh records' }));
    await waitFor(() => expect(screen.getByText('M31')).toBeInTheDocument());
  });
});
