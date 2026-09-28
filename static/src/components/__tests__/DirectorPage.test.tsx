import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import { useScopedDbId } from '../../hooks/useUrlState';
import DirectorPage from '../director/DirectorPage';
import type { DirectorPlanRow } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const record = { id: '11111111-1111-4111-8111-111111111111', name: 'M31', revision: 1 };
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
const enabled = { protocol_version: 1, enabled: true, instance_id: record.id, acquisition_available: false };
const row = (project = record, extra: Partial<DirectorPlanRow> = {}): DirectorPlanRow => ({ project, links: [], progress: null, framing: null, plan: null, activation: null, ...extra });
const list = (rows: DirectorPlanRow[], warnings: string[] = []) => ({ rows, warnings });
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
    // Plan thumbnails: a survey image; the panels are drawn from the framing itself.
    http.get('/api/director/v1/sky/cutout', () => HttpResponse.arrayBuffer(new Uint8Array([255, 216, 255]).buffer, { status: 200, headers: { 'content-type': 'image/jpeg' } })),
  );
  render(<DirectorPage />, { wrapper: Wrapper });
}

describe('Director management', () => {
  beforeEach(() => { vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => 'blob:thumb'), revokeObjectURL: vi.fn() })); });
  afterEach(() => vi.unstubAllGlobals());

  it('creates with the same UUID after an ambiguous failure, and trims names', async () => {
    const requests: { id: string; name: string }[] = [];
    let committed = false;
    server.use(
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list(committed ? [row({ ...record, ...requests[0] })] : [])))),
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
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([row(current)])))),
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
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([
        row(record, { links: [{ catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'g', source_row_id: 7, source_name: 'Andromeda subs',
          targets: [{ name: 'M31 r1c1', desired: 72, acquired: 40, accepted: 36, rejected: 3, center: { ra_degrees: 10.68, dec_degrees: 41.27 }, rotation_degrees: 35 }, { name: 'M31 r2c1', desired: 72, acquired: 0, accepted: 0, rejected: 0, center: null, rotation_degrees: null }] }],
          progress: { desired: 144, acquired: 40, accepted: 36, rejected: 3, targets: 2 },
          framing: { source: 'draft', revision: 2, target_name: 'M31', panels: 4, panel_rig_id: rig.id, center: { ra_degrees: 10.68, dec_degrees: 41.27 }, position_angle_degrees: 35, panel: { width_degrees: 1, height_degrees: 0.75 }, mosaic: { rows: 2, columns: 2, overlap_percent: 20 }, survey_id: 'dss2_color', extent: { width_degrees: 1.8, height_degrees: 1.35 } }, plan: { revision: 1, objectives: 2, rigs: 1 }, activation: null }),
        row({ ...record, id: '33333333-3333-4333-8333-333333333333', name: 'Bare' }),
        row({ ...record, id: '44444444-4444-4444-8444-444444444444', name: 'Pelican' }, { links: [{ catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'p', source_row_id: 9, source_name: 'Pelican', targets: [{ name: 'IC 5070', desired: 40, acquired: 12, accepted: 10, rejected: 0, center: { ra_degrees: 312.75, dec_degrees: 44.37 }, rotation_degrees: 90 }] }],
          progress: { desired: 40, acquired: 12, accepted: 10, rejected: 0, targets: 1 },
          framing: { source: 'catalog', revision: 0, target_name: 'IC 5070', panels: 1, panel_rig_id: rig.id, center: { ra_degrees: 312.75, dec_degrees: 44.37 }, position_angle_degrees: 90, panel: { width_degrees: 0.7, height_degrees: 0.5 }, mosaic: { rows: 1, columns: 1, overlap_percent: 20 }, survey_id: 'dss2_color', extent: { width_degrees: 0.7, height_degrees: 0.5 } } }),
      ], ['Odd file: has no Target Scheduler project table'])))),
      http.get('/api/director/v1/rigs/profiles', () => HttpResponse.json(ok([{ rig, catalog_slug: 'c925', catalog_name: 'C925', profile: null,
        field_of_view: { width_degrees: 0.7, height_degrees: 0.5, pixel_scale_arcsec: 0.41, focal_ratio: 10 }, default_exposure_seconds: { broadband: 120, narrowband: 300 } }]))),
      http.get('/api/director/v1/rigs/status', () => HttpResponse.json(ok([{ rig, catalog_slug: 'c925', catalog_name: 'C925', checkins: [], status: { rig_id: rig.id, session_id: 's1', reported_at_ms: 1_700_000_000_000, received_at_ms: 1_700_000_000_001, payload: { phase: 'exposing' } },
        status_age_ms: 5000, status_stale: false, contacts: { program_pull: null, check_in: null, status: { at_ms: 1_700_000_000_001, detail: 's1' } }, connectivity: { state: 'online', last_contact_ms: 1_700_000_000_001, age_ms: 5000 }, assignments: [], pending_receipts: 0 }]))),
    ]);
    expect(await screen.findByText('M31')).toBeInTheDocument();
    expect(screen.getByText('Planned: 2 objectives, 1 rig, not activated')).toBeInTheDocument();
    // The card reads like an Overview project: counts, bars, then one row per rig.
    expect(screen.getByText('36 / 144 desired')).toBeInTheDocument();
    expect(screen.getAllByText('25% complete')).toHaveLength(2);
    expect(screen.getByRole('img', { name: 'Grading status: 36 accepted, 3 rejected, 1 pending' })).toBeInTheDocument();
    const rigs = screen.getByRole('region', { name: 'Rigs shooting M31' });
    expect(within(rigs).getByText('Andromeda subs')).toBeInTheDocument();
    expect(within(rigs).getByText('36/144 frames · 2 targets · 25%')).toBeInTheDocument();
    expect(within(rigs).getByText('M31 r1c1 36/72 frames · M31 r2c1 0/72 frames')).toBeInTheDocument();
    // A framed plan shows its survey thumbnail with the panels drawn on it.
    const thumb = screen.getByRole('img', { name: 'Framing of M31 on dss2 color' });
    await waitFor(() => expect(thumb.querySelector('img')).toBeInTheDocument());
    expect(thumb.querySelectorAll('.plan-thumb-panel')).toHaveLength(4);
    expect(screen.queryByRole('img', { name: /Framing of Bare/ })).not.toBeInTheDocument();
    // A project Target Scheduler already points somewhere is framed there, thumbnail and all.
    expect(screen.getByText('Framed in Target Scheduler: IC 5070; open to plan it in Director')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'Framing of Pelican on dss2 color' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open M31' })).toHaveAttribute('href', `/director?db=old-catalog&project=123&directorProject=${record.id}`);
    expect(screen.getByText('Not linked to any database')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open Bare' })).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('Odd file: has no Target Scheduler project table');
    expect(screen.queryByRole('button', { name: /New project|Rename/ })).not.toBeInTheDocument();
    expect(await screen.findByText(/Field 42.0′ × 30.0′, 0.41″\/px, camera not reported yet/)).toBeInTheDocument();
    expect(screen.getByText(/Plugin: exposing/)).toBeInTheDocument();
    expect(screen.getByText('Online')).toBeInTheDocument();
    expect(screen.getByText('nothing activated')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Setup C925' })).toBeInTheDocument();
    expect(screen.getByTestId('location')).toHaveTextContent('?db=old-catalog&project=123|scope=global');
  });

  it('folds the old tab links into the one page while keeping catalog URL state', async () => {
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([])))));
    mount(true, '/director?db=old-catalog&project=123&directorView=sites');
    expect(await screen.findByText('C925')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Plans' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'New rig' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Sites' })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('?db=old-catalog&project=123&directorView=projects|scope=global'));
  });

  it.each([{ ...enabled, enabled: false }, { ...enabled, protocol_version: 2 }])('does not load records when unavailable: %j', async status => {
    const plansCall = vi.fn(() => HttpResponse.json(ok(list([]))));
    mount();
    server.use(http.get('/api/director/v1/status', () => HttpResponse.json(ok(status))), http.get('/api/director/v1/plans', plansCall));
    expect(await screen.findByText('Director management is unavailable on this server.')).toBeInTheDocument();
    expect(plansCall).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'New project' })).not.toBeInTheDocument();
  });

  it('rejects overlong multibyte names before sending them', async () => {
    const create = vi.fn(() => HttpResponse.json(ok(record)));
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([])))), http.post('/api/director/v1/projects', create));
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
      ? HttpResponse.json({ error: 'Director metadata operation failed; see server logs' }, { status: 500 })
      : HttpResponse.json(ok(list([row()])))));
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent('failed');
    expect(screen.queryByText('No projects yet.')).not.toBeInTheDocument();
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh records' }));
    await waitFor(() => expect(screen.getByText('M31')).toBeInTheDocument());
  });

  it('waits through a busy answer instead of showing it as an error', async () => {
    let calls = 0;
    server.use(http.get('/api/director/v1/plans', () => ++calls === 1
      ? HttpResponse.json({ error: 'Director metadata is busy; retry shortly' }, { status: 503 })
      : HttpResponse.json(ok(list([row()])))));
    mount();
    expect(await screen.findByText('M31', {}, { timeout: 6000 })).toBeInTheDocument();
    expect(calls).toBe(2);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByText('No projects yet.')).not.toBeInTheDocument();
  });
});
