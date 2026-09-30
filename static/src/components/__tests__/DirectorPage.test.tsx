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
const enabled = { protocol_version: 1, enabled: true, instance_id: record.id, acquisition_available: false, database_management: true };
const row = (project = record, extra: Partial<DirectorPlanRow> = {}): DirectorPlanRow => ({ project, links: [], progress: null, framing: null, plan: null, activation: null, ...extra });
const list = (rows: DirectorPlanRow[], warnings: string[] = []) => ({ rows, warnings });
/** The display preference module reads storage once and again on a storage
 *  event, so set the density the way another tab would. */
function chooseDensity(density: 'compact' | 'detailed' | null) {
  if (density) localStorage.setItem('psf-guard.display-preferences', JSON.stringify({ libraryDensity: density }));
  else localStorage.removeItem('psf-guard.display-preferences');
  window.dispatchEvent(new StorageEvent('storage', { key: 'psf-guard.display-preferences' }));
}
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
  afterEach(() => { vi.unstubAllGlobals(); chooseDensity(null); });

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

  it('lists plans as Library rows with state, progress and rigs, and folds one open to its card', async () => {
    const target = (name: string, desired: number, acquired: number, accepted: number, rejected: number) => ({ name, desired, acquired, accepted, rejected, center: null, rotation_degrees: null });
    const day = 86_400;
    const now = Math.floor(Date.now() / 1000);
    const other = { id: '55555555-5555-4555-8555-555555555555', name: 'RC51', revision: 1 };
    mount(false, undefined, [
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([
        row(record, { links: [
          { catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'g', source_row_id: 7, source_name: 'M31', source_state: 1, earliest_capture_s: now - 30 * day, latest_capture_s: now - 3 * day, targets: [target('M31', 72, 40, 36, 3)] },
          { catalog_slug: 'rc51', catalog_name: 'RC51', rig: other, source_project_guid: 'g', source_row_id: 3, source_name: 'M31', source_state: 2, earliest_capture_s: now - 60 * day, latest_capture_s: now - 20 * day, targets: [target('M31', 10, 12, 10, 2)] },
        ], progress: { desired: 82, acquired: 52, accepted: 46, rejected: 5, targets: 2 }, plan: { revision: 1, objectives: 2, rigs: 2 } }),
        row({ ...record, id: '33333333-3333-4333-8333-333333333333', name: 'Bare' }),
        row({ ...record, id: '44444444-4444-4444-8444-444444444444', name: 'Pelican' }, { links: [{ catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'p', source_row_id: 9, source_name: 'Pelican', source_state: 3, earliest_capture_s: null, latest_capture_s: null, targets: [target('IC 5070', 40, 12, 10, 0)] }],
          progress: { desired: 40, acquired: 12, accepted: 10, rejected: 0, targets: 1 },
          framing: { source: 'catalog', revision: 0, target_name: 'IC 5070', panels: 1, panel_rig_id: rig.id, center: { ra_degrees: 312.75, dec_degrees: 44.37 }, position_angle_degrees: 90, panel: { width_degrees: 0.7, height_degrees: 0.5 }, mosaic: { rows: 1, columns: 1, overlap_percent: 20 }, survey_id: 'dss2_color', extent: { width_degrees: 0.7, height_degrees: 0.5 } } }),
      ])))),
    ]);
    // A plan shot by two rigs is an outer pill with a row per rig, each in the Library's pills.
    const family = await screen.findByTestId('plan-family');
    expect(within(family).getByRole('link', { name: 'Open M31' })).toBeInTheDocument();
    expect(within(family).getByText('2 rigs')).toBeInTheDocument();
    expect(within(family).getByText('Planned')).toBeInTheDocument();
    expect(within(family).getByText(/46 \/ 82 · 56%/)).toBeInTheDocument();
    const members = within(family).getAllByTestId('plan-row');
    expect(members).toHaveLength(2);
    expect(within(members[0]).getByText('C925')).toBeInTheDocument();
    expect(within(members[0]).getByText('Active')).toBeInTheDocument();
    expect(within(members[0]).getByText(/36 \/ 72 · 50%/)).toBeInTheDocument();
    expect(within(members[0]).getByText(/3 d ago/)).toBeInTheDocument();
    expect(within(members[1]).getByText('Inactive')).toBeInTheDocument();
    expect(within(members[1]).getByText(/10 \/ 10 · Done/)).toHaveClass('is-done');
    // A plan with no database says so where the pills would be.
    expect(screen.getByText('Not linked to any database')).toBeInTheDocument();
    // Closed on every rig that shoots it: behind the Library's archive fold, then one row.
    expect(screen.queryByText('Closed', { selector: '.library-pill' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Archived plans/ }));
    expect(screen.getByText('Closed', { selector: '.library-pill' })).toBeInTheDocument();
    expect(screen.getByText('Framed in Target Scheduler')).toBeInTheDocument();
    expect(screen.getByText('No dates')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open Bare' })).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: 'Rigs shooting M31' })).not.toBeInTheDocument();
    // The arrow opens the full card and folds it back.
    fireEvent.click(screen.getByRole('button', { name: 'Show details for M31' }));
    expect(screen.getByRole('region', { name: 'Rigs shooting M31' })).toBeInTheDocument();
    expect(screen.queryByTestId('plan-family')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Hide details for M31' }));
    expect(screen.queryByRole('region', { name: 'Rigs shooting M31' })).not.toBeInTheDocument();
    // Detailed opens every plan as its card, as it does in the Library.
    fireEvent.click(screen.getByRole('radio', { name: 'Detailed' }));
    expect(screen.getByRole('region', { name: 'Rigs shooting M31' })).toBeInTheDocument();
    expect(screen.getByText('Framed in Target Scheduler: IC 5070; open to plan it in Director')).toBeInTheDocument();
    expect(screen.queryAllByTestId('plan-row')).toHaveLength(0);
    fireEvent.click(screen.getByRole('radio', { name: 'Compact' }));
    expect(screen.getAllByTestId('plan-row')).toHaveLength(4);
  });

  it('narrows the list by state or search in the URL, opens each rig in the Library or the grid, and edits a rig state', async () => {
    const target = (name: string, desired: number, acquired: number, accepted: number, rejected: number) => ({ name, desired, acquired, accepted, rejected, center: null, rotation_degrees: null });
    const other = { id: '55555555-5555-4555-8555-555555555555', name: 'RC51', revision: 1 };
    const puts: { url: string; body: unknown }[] = [];
    let c925State = 1;
    mount(true, undefined, [
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([
        row(record, { links: [
          { catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'g', source_row_id: 7, source_name: 'M31', source_state: c925State, earliest_capture_s: null, latest_capture_s: null, targets: [target('M31', 72, 40, 36, 3)] },
          { catalog_slug: 'rc51', catalog_name: 'RC51', rig: other, source_project_guid: 'g', source_row_id: 3, source_name: 'M31', source_state: 2, earliest_capture_s: null, latest_capture_s: null, targets: [target('M31', 10, 12, 10, 2)] },
        ] }),
        row({ ...record, id: '33333333-3333-4333-8333-333333333333', name: 'Bare' }),
        row({ ...record, id: '44444444-4444-4444-8444-444444444444', name: 'Pelican' }, { links: [{ catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'p', source_row_id: 9, source_name: 'Pelican', source_state: 1, earliest_capture_s: null, latest_capture_s: null, targets: [target('IC 5070', 40, 40, 40, 0)] }] }),
      ])))),
      http.put('/api/db/c925/projects/7', async ({ request }) => { puts.push({ url: new URL(request.url).pathname, body: await request.json() }); c925State = 2; return HttpResponse.json(ok({ updated: true })); }),
    ]);
    expect(await screen.findByTestId('plan-family')).toBeInTheDocument();
    expect(screen.getAllByTestId('plan-row')).toHaveLength(4);
    // Every rig's project opens in the Library or the image grid from its row.
    expect(screen.getAllByRole('link', { name: 'Show M31 in the Library' })[0]).toHaveAttribute('href', '/?db=c925&project=7');
    expect(screen.getAllByRole('link', { name: 'Show M31 images' })[0]).toHaveAttribute('href', '/grid?db=c925&project=7');
    expect(screen.getByRole('link', { name: 'Show Pelican images' })).toHaveAttribute('href', '/grid?db=c925&project=9');
    // The filter is URL state: Inactive keeps the plan whose RC51 rig is inactive; No database keeps Bare.
    fireEvent.change(screen.getByRole('combobox', { name: 'Show plans' }), { target: { value: 'inactive' } });
    expect(screen.getByTestId('location')).toHaveTextContent('directorShow=inactive');
    expect(screen.getByTestId('plan-family')).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Open Bare' })).not.toBeInTheDocument();
    expect(screen.getByText('Showing 1 of 3 plans.')).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: 'Show plans' }), { target: { value: 'done' } });
    expect(screen.getByRole('link', { name: 'Open Pelican' })).toBeInTheDocument();
    expect(screen.queryByTestId('plan-family')).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: 'Show plans' }), { target: { value: 'unlinked' } });
    expect(screen.getByRole('link', { name: 'Open Bare' })).toBeInTheDocument();
    expect(screen.getAllByTestId('plan-row')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Show all' }));
    expect(screen.getByTestId('location')).not.toHaveTextContent('directorShow');
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search plans' }), { target: { value: 'peli' } });
    expect(screen.getByTestId('location')).toHaveTextContent('directorSearch=peli');
    expect(screen.getAllByTestId('plan-row')).toHaveLength(1);
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search plans' }), { target: { value: '' } });
    // With database management, the state pill is a select that writes the rig's project row.
    fireEvent.change(screen.getByRole('combobox', { name: 'State of M31 in C925' }), { target: { value: '2' } });
    expect(await screen.findByText('M31 is now Inactive on C925.')).toBeInTheDocument();
    expect(puts).toEqual([{ url: '/api/db/c925/projects/7', body: { state: 2 } }]);
    await waitFor(() => expect(screen.getByRole('combobox', { name: 'State of M31 in C925' })).toHaveValue('2'));
  });

  it('shows each plan with its stage and opens planning at its linked database row', async () => {
    // The detailed view shows every plan as its full card.
    chooseDensity('detailed');
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
    // The card reads like a Library project: counts, bars, then one row per rig.
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
    // A read-only viewer cannot open Settings, so rigs and templates stay on the page for them.
    expect(await screen.findByText(/Field 42.0′ × 30.0′, 0.41″\/px, camera not reported yet/)).toBeInTheDocument();
    expect(screen.getByText(/Plugin: exposing/)).toBeInTheDocument();
    expect(screen.getByText('Online')).toBeInTheDocument();
    expect(screen.getByText('nothing activated')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Setup C925' })).toBeInTheDocument();
    expect(screen.queryByText(/are under Settings/)).not.toBeInTheDocument();
    expect(screen.getByTestId('location')).toHaveTextContent('?db=old-catalog&project=123|scope=global');
  });

  it('folds the old tab links into the one page while keeping catalog URL state', async () => {
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([])))));
    mount(true, '/director?db=old-catalog&project=123&directorView=sites');
    expect(await screen.findByRole('heading', { name: 'Plans' })).toBeInTheDocument();
    // An editor finds rig setup and templates under Settings; the page points there.
    expect(screen.getByRole('button', { name: 'Rigs' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Exposure templates' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Setup C925' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'New rig' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Sites' })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('?db=old-catalog&project=123&directorView=projects|scope=global'));
  });

  it.each([{ ...enabled, enabled: false }, { ...enabled, protocol_version: 2 }])('does not load records when unavailable: %j', async status => {
    const plansCall = vi.fn(() => HttpResponse.json(ok(list([]))));
    mount();
    server.use(http.get('/api/director/v1/status', () => HttpResponse.json(ok(status))), http.get('/api/director/v1/plans', plansCall));
    expect(await screen.findByText('Planning is unavailable on this server.')).toBeInTheDocument();
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
