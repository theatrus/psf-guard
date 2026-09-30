import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import LibraryPlans from '../director/LibraryPlans';
import type { LibraryShow } from '../libraryShow';
import type { DirectorPlanRow } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const record = { id: '11111111-1111-4111-8111-111111111111', name: 'M31', revision: 1 };
const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
const enabled = { protocol_version: 1, enabled: true, instance_id: record.id, acquisition_available: false, database_management: true };
const row = (project = record, extra: Partial<DirectorPlanRow> = {}): DirectorPlanRow => ({ project, links: [], progress: null, framing: null, plan: null, activation: null, ...extra });
const list = (rows: DirectorPlanRow[], warnings: string[] = []) => ({ rows, warnings });

function mount(canWrite = true, props: { search?: string; show?: LibraryShow; listed?: Set<string> } = {}, status: unknown = enabled) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}><MemoryRouter initialEntries={['/?db=c925&project=3']}>{children}</MemoryRouter></AccessContext.Provider></QueryClientProvider>;
  }
  server.use(http.get('/api/director/v1/status', () => HttpResponse.json(ok(status))));
  return render(<LibraryPlans search={props.search ?? ''} show={props.show ?? 'all'} listed={props.listed ?? new Set()} />, { wrapper: Wrapper });
}

describe("The Library's plans with nothing captured yet", () => {
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
    fireEvent.click(await screen.findByRole('button', { name: 'New plan' }));
    fireEvent.change(screen.getByLabelText('Plan name'), { target: { value: '  Sky survey  ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Network Error');
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Created Sky survey.')).toBeInTheDocument();
    expect(requests).toHaveLength(2);
    expect(requests[0]).toEqual(requests[1]);
    expect(requests[0].name).toBe('Sky survey');
    expect(requests[0].id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(await screen.findByRole('link', { name: 'Open Sky survey' })).toHaveAttribute('href', `/plan?db=c925&project=3&plan=${requests[0].id}`);
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
    fireEvent.change(screen.getByLabelText('Plan name'), { target: { value: 'Andromeda' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Revision conflict');
    expect(screen.getByLabelText('Plan name')).toHaveValue('Andromeda');
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh plans' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Rename Changed elsewhere' }));
    fireEvent.change(screen.getByLabelText('Plan name'), { target: { value: 'Andromeda' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Renamed Andromeda.')).toBeInTheDocument();
    expect(updates).toEqual([{ name: 'Andromeda', expected_revision: 1 }, { name: 'Andromeda', expected_revision: 2 }]);
  });

  it('rejects overlong multibyte names before sending them', async () => {
    const create = vi.fn(() => HttpResponse.json(ok(record)));
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([])))), http.post('/api/director/v1/projects', create));
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'New plan' }));
    fireEvent.change(screen.getByLabelText('Plan name'), { target: { value: 'é'.repeat(300) } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('512 UTF-8 bytes');
    expect(create).not.toHaveBeenCalled();
  });

  it('lists plans the Library has no row for, narrowed by search and Show', async () => {
    const linked = row({ ...record, id: '33333333-3333-4333-8333-333333333333', name: 'Linked' }, { links: [{ catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'g', source_row_id: 7, source_name: 'Linked', source_state: 1, earliest_capture_s: null, latest_capture_s: null, targets: [] }] });
    const framed = row({ ...record, id: '44444444-4444-4444-8444-444444444444', name: 'Pelican' }, { framing: { source: 'draft', revision: 1, target_name: 'IC 5070', panels: 1, panel_rig_id: null, center: null, position_angle_degrees: 0, panel: null, mosaic: { rows: 1, columns: 1, overlap_percent: 20 }, survey_id: 'dss2_color', extent: null } as unknown as DirectorPlanRow['framing'] });
    const waitingLinked = row({ ...record, id: '55555555-5555-4555-8555-555555555555', name: 'Waiting' }, { links: [{ catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'w', source_row_id: 8, source_name: 'Waiting', source_state: 2, earliest_capture_s: null, latest_capture_s: null, targets: [] }] });
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([row(), linked, framed, waitingLinked], ['Odd file: has no Target Scheduler project table'])))));
    // The Library has a row for Linked (it has frames) but none for Waiting.
    const view = mount(false, { listed: new Set(['c925:7']) });
    expect(await screen.findByRole('link', { name: 'Open M31' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open Pelican' })).toBeInTheDocument();
    expect(screen.getByText('Framed')).toBeInTheDocument();
    expect(screen.getAllByText('Not linked to any database')).toHaveLength(2);
    expect(screen.queryByRole('link', { name: 'Open Linked' })).not.toBeInTheDocument();
    // A plan activated on a rig that has captured nothing waits here with its rig and state.
    const waiting = screen.getAllByTestId('plan-row').find(item => item.textContent?.includes('Waiting'))!;
    expect(waiting).toHaveTextContent('C925');
    expect(waiting).toHaveTextContent('Inactive');
    expect(waiting).toHaveTextContent('No frames yet');
    expect(screen.getByRole('alert')).toHaveTextContent('Odd file: has no Target Scheduler project table');
    // A viewer sees plans but cannot start or rename one.
    expect(screen.queryByRole('button', { name: /New plan|Rename/ })).not.toBeInTheDocument();
    const listed = new Set(['c925:7']);
    view.rerender(<LibraryPlans search="peli" show="all" listed={listed} />);
    expect(screen.queryByRole('link', { name: 'Open M31' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open Pelican' })).toBeInTheDocument();
    // Show applies to the rigs' states: Inactive keeps Waiting and drops plans without a database.
    view.rerender(<LibraryPlans search="" show="inactive" listed={listed} />);
    expect(screen.queryByRole('link', { name: 'Open Pelican' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open Waiting' })).toBeInTheDocument();
    view.rerender(<LibraryPlans search="" show="unlinked" listed={listed} />);
    expect(screen.queryByRole('link', { name: 'Open Waiting' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open Pelican' })).toBeInTheDocument();
  });

  it('leaves linked plans out when a database could not be read', async () => {
    const waitingLinked = row({ ...record, id: '55555555-5555-4555-8555-555555555555', name: 'Waiting' }, { links: [{ catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'w', source_row_id: 8, source_name: 'Waiting', source_state: 1, earliest_capture_s: null, latest_capture_s: null, targets: [] }] });
    server.use(http.get('/api/director/v1/plans', () => HttpResponse.json(ok(list([row(), waitingLinked])))));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    server.use(http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))));
    render(<QueryClientProvider client={client}><MemoryRouter><LibraryPlans search="" show="all" listed={new Set()} incomplete /></MemoryRouter></QueryClientProvider>);
    expect(await screen.findByRole('link', { name: 'Open M31' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Open Waiting' })).not.toBeInTheDocument();
    expect(screen.getByText('Some databases could not be read, so their plans are left out here.')).toBeInTheDocument();
  });

  it('recovers a failed listing without leaving a false empty state', async () => {
    let fail = true;
    server.use(http.get('/api/director/v1/plans', () => fail
      ? HttpResponse.json({ error: 'Director metadata operation failed; see server logs' }, { status: 500 })
      : HttpResponse.json(ok(list([row()])))));
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent('failed');
    expect(screen.queryByText('Every plan has frames.')).not.toBeInTheDocument();
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh plans' }));
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
  });

  it('stays out of the Library when Planning is off', async () => {
    const plansCall = vi.fn(() => HttpResponse.json(ok(list([row()]))));
    server.use(http.get('/api/director/v1/plans', plansCall));
    const view = mount(true, {}, { ...enabled, enabled: false });
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(view.container).toBeEmptyDOMElement();
    expect(plansCall).not.toHaveBeenCalled();
  });
});
