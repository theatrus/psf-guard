import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../../test/msw-server';
import RigsPage from '../../rigs/RigsPage';
import { night, plans, rig } from './fixtures';
import { formatNightDate } from '../nightsModel';

const ok = (data: unknown) => ({ success: true, data, error: null });
const enabled = { protocol_version: 1, enabled: true, instance_id: '11111111-1111-4111-8111-111111111111', database_management: true };

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}{location.search}</output>;
}

function mount(body = plans(), route = '/rigs?rigsTab=nights') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const planned = vi.fn(() => HttpResponse.json(ok(body)));
  const writes: Array<{ url: string; body: { expected_revision: number; marks: Array<{ state: string; note: string; windows: Array<{ start_ms: number }> }> } }> = [];
  const saved = vi.fn(async ({ request }: { request: Request }) => {
    writes.push({ url: request.url, body: await request.json() });
    return HttpResponse.json(ok({ scope: 'site', scope_id: 'x', revision: 1, marks: [] }));
  });
  server.use(
    http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))),
    http.get('/api/director/v1/plans', () => HttpResponse.json(ok({ rows: [] }))),
    http.post('/api/director/v1/night-plans', planned),
    http.put('/api/director/v1/night-availability/:scope/:id', saved),
    http.get('/api/director/v1/rigs/status', () => HttpResponse.json(ok([]))),
  );
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></QueryClientProvider>;
  return { ...render(<RigsPage />, { wrapper: Wrapper }), planned, saved, writes };
}

describe('Rigs', () => {
  it('opens on Live and switches to Nights, keeping the tab in the address', async () => {
    mount(plans(), '/rigs');
    expect(await screen.findByRole('tab', { name: 'Live' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('link', { name: /Show the rigs on the sky/ })).toHaveAttribute('href', '/sky?live=1');
    fireEvent.click(screen.getByRole('tab', { name: 'Nights' }));
    expect(await screen.findByTestId('nights-grid')).toBeInTheDocument();
    expect(screen.getByTestId('location')).toHaveTextContent('/rigs?rigsTab=nights');
  });
});

describe('Nights', () => {
  it('shows each rig across its nights and the chosen night in detail', async () => {
    const { planned } = mount();
    const grid = await screen.findByTestId('nights-grid');
    expect(planned).toHaveBeenCalled();
    expect(within(grid).getByText('RedCat 61')).toBeInTheDocument();
    expect(within(grid).getByText('Target Scheduler · Backyard')).toBeInTheDocument();
    const cells = within(grid).getAllByRole('button');
    expect(cells).toHaveLength(3);
    expect(cells[1]).toHaveAccessibleName(/none clear of 10\.0 h dark, not available, Moon 42%/);
    expect(within(cells[0]).getByText('5.0 h idle')).toBeInTheDocument();
    fireEvent.click(cells[1]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText(/Rig: Not available/)).toBeInTheDocument();
    expect(within(detail).getByText('· dome motor')).toBeInTheDocument();
    // Marked out, the night applies to nothing; the target folds away
    // with its reason.
    const plansTonight = within(detail).getByRole('region', { name: 'No owed target can be imaged this night' });
    expect(within(plansTonight).getByText('Not this night: 1 target')).toBeInTheDocument();
    expect(within(plansTonight).getByRole('listitem')).toHaveTextContent('Heart Nebula · IC 1805: above its limits only in hours that are not clear');
    expect(cells[1]).toHaveAttribute('aria-pressed', 'true');
    // A clear night lists it, usable for its hours less the Moon's. With
    // no schedule to say what is planned, nothing is called waiting.
    fireEvent.click(cells[0]);
    const clearNight = within(screen.getByRole('region', { name: 'Night details' })).getByRole('region', { name: 'This night: 1 target up and clear' });
    const row = within(clearNight).getByRole('row', { name: /IC 1805/ });
    expect(row).toHaveTextContent('5.0 h · 1.0 h excluded by the Moon');
    expect(row).not.toHaveClass('is-idle-plan');
  });

  it('shows a night the site\'s forecast marked, credits it, and leaves the mark form to marks by hand', async () => {
    const fetched = night(0).noon_ms + 2 * 3_600_000;
    const forecastNight = night(0, {
      available_dark_hours: 4,
      availability: { state: 'limited', windows: [{ start_ms: night(0).noon_ms + 9 * 3_600_000, end_ms: night(0).noon_ms + 13 * 3_600_000 }],
        marks: [{ scope: 'site', state: 'limited', windows: [{ start_ms: night(0).noon_ms + 9 * 3_600_000, end_ms: night(0).noon_ms + 13 * 3_600_000 }], note: '', source: { kind: 'forecast', provider: 'open_meteo', fetched_at_ms: fetched } }] },
      clouds: [{ start_ms: night(0).noon_ms + 8 * 3_600_000, end_ms: night(0).noon_ms + 9 * 3_600_000, cloud_total: 80, cloud_low: 70, cloud_mid: null, cloud_high: 10 }],
    });
    mount(plans([rig({
      nights: [forecastNight, night(1), night(2)],
      forecast: { provider: 'open_meteo', attribution: 'Weather data by Open-Meteo.com (CC BY 4.0)', cloud_threshold_percent: 30, fetched_at_ms: fetched, expires_at_ms: fetched + 12 * 3_600_000 },
    })]));
    const grid = await screen.findByTestId('nights-grid');
    const cells = within(grid).getAllByRole('button');
    expect(cells[0]).toHaveAccessibleName(/limited by the forecast/);
    expect(cells[1]).not.toHaveAccessibleName(/forecast/);
    fireEvent.click(cells[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText(/^Forecast \(Open-Meteo, fetched .+\): Limited/)).toBeInTheDocument();
    expect(within(detail).getByText(/Hours at or under 30% cloud count as clear/)).toBeInTheDocument();
    expect(within(detail).getByRole('link', { name: 'Weather data by Open-Meteo.com (CC BY 4.0)' })).toHaveAttribute('href', 'https://open-meteo.com/');
    // The forecast's mark is not one to edit or clear.
    fireEvent.change(within(detail).getByLabelText('For'), { target: { value: 'site' } });
    expect(within(detail).getByLabelText('Not available')).toBeChecked();
    expect(within(detail).queryByRole('button', { name: 'Clear mark' })).toBeNull();
  });

  it('says when the site\'s forecast does not reach a night', async () => {
    mount(plans([rig({ forecast: { provider: 'met_norway', attribution: 'Weather data from MET Norway (CC BY 4.0)', cloud_threshold_percent: 30, fetched_at_ms: null, expires_at_ms: null } })]));
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[2]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText(/forecast from MET Norway has not arrived or has expired, so nights count as clear/)).toBeInTheDocument();
  });

  it('marks a run of nights limited for every rig at the site', async () => {
    const { saved, writes } = mount();
    const grid = await screen.findByTestId('nights-grid');
    const cells = within(grid).getAllByRole('button');
    fireEvent.click(cells[0]);
    fireEvent.click(cells[2], { shiftKey: true });
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText('Mark these 3 nights')).toBeInTheDocument();
    // The scope first: each scope's form starts from that scope's mark.
    fireEvent.change(within(detail).getByLabelText('For'), { target: { value: 'site' } });
    fireEvent.click(within(detail).getByLabelText('Limited'));
    fireEvent.change(within(detail).getByLabelText('From'), { target: { value: '22:30' } });
    fireEvent.change(within(detail).getByLabelText('Note'), { target: { value: 'high cirrus' } });
    fireEvent.click(within(detail).getByRole('button', { name: 'Save mark' }));
    await waitFor(() => expect(saved).toHaveBeenCalled());
    const [{ url, body }] = writes;
    expect(url).toContain(`/night-availability/site/${rig().planning_site!.id}`);
    expect(body.expected_revision).toBe(0);
    expect(body.marks).toHaveLength(3);
    for (const mark of body.marks) {
      expect(mark.state).toBe('limited');
      expect(mark.note).toBe('high cirrus');
      expect(mark.windows).toHaveLength(1);
      expect(new Date(mark.windows[0].start_ms).getHours()).toBe(22);
    }
  });

  it('starts each scope\'s form from that scope\'s own mark', async () => {
    mount();
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[1]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    // The rig's own mark: not available, with its note.
    expect(within(detail).getByLabelText('Not available')).toBeChecked();
    expect(within(detail).getByLabelText('Note')).toHaveValue('dome motor');
    // The site has no mark that night: its form starts empty.
    fireEvent.change(within(detail).getByLabelText('For'), { target: { value: 'site' } });
    expect(within(detail).getByLabelText('Note')).toHaveValue('');
  });

  it('lines rigs up by date when one is a night ahead of another', async () => {
    const near = rig();
    const far = rig({
      rig: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'Siding Spring', revision: 1 },
      catalog_name: 'Far rig',
      nights: [night(1), night(2), night(3)],
    });
    mount(plans([near, far]));
    const grid = await screen.findByTestId('nights-grid');
    // Four dates: the near rig's three and one more.
    expect(within(grid).getAllByRole('columnheader')).toHaveLength(5);
    const farRow = within(grid).getByRole('row', { name: /Far rig/ });
    const cells = within(farRow).getAllByRole('cell');
    expect(cells).toHaveLength(4);
    expect(within(cells[0]).queryByRole('button')).toBeNull();
    expect(within(cells[1]).getByRole('button')).toHaveAccessibleName(new RegExp(`^Far rig, ${formatNightDate(night(1).date)}`));
  });

  it('lets a reader see the nights but not mark them', async () => {
    const { AccessContext } = await import('../../../auth/access');
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    server.use(
      http.get('/api/director/v1/status', () => HttpResponse.json(ok(enabled))),
      http.get('/api/director/v1/plans', () => HttpResponse.json(ok({ rows: [] }))),
      http.post('/api/director/v1/night-plans', () => HttpResponse.json(ok(plans()))),
    );
    render(<AccessContext.Provider value={{ status: { authentication_required: true, authenticated: true, role: 'read_only', can_compute: true }, canWrite: false, canCompute: true, logout: async () => undefined }}>
      <QueryClientProvider client={client}><MemoryRouter initialEntries={['/rigs?rigsTab=nights']}><RigsPage /></MemoryRouter></QueryClientProvider>
    </AccessContext.Provider>);
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).queryByRole('button', { name: 'Save mark' })).toBeNull();
  });
});

describe('Night overrides', () => {
  const HOUR = 3_600_000;

  /** A Director rig whose night 0 is scheduled, on a UTC site clock. */
  function directorRig() {
    const base = night(0);
    const block = {
      target: { project_id: 7, project_name: 'Heart Nebula', target_id: 70, target_name: 'IC 1805' },
      start_ms: base.dusk_ms!, end_ms: base.dusk_ms! + 2 * HOUR, slew: true,
      runs: [{ exposure_plan_id: 700, filter: 'Ha', exposure_seconds: 300, frames: 24, dithers: 0, start_ms: base.dusk_ms!, end_ms: base.dusk_ms! + 2 * HOUR }],
      decision: { kind: 'ranked' as const, candidates: [] },
    };
    return rig({
      runtime: 'director',
      time_zone: 'UTC',
      schedule: {
        runtime: 'director',
        nights: [{ date: base.date, start_ms: base.noon_ms, end_ms: base.noon_ms + 24 * HOUR, blocks: [block], gaps: [], frames: 24, exposure_hours: 2, note: null }],
        progress: [],
        assumptions: { target_scheduler_version: null, fidelity: 'estimate', overheads: null, acceptance: 'plan_rate', utc_offset_minutes: -473 },
        goals: [{ goal_id: 'e1', plan_id: 'p1', plan_name: 'Heart Nebula', target_guid: 't1', exposureplan_guid: 'e1', project_id: 7, target_id: 70, exposure_plan_id: 700, target_name: 'IC 1805', filter: 'Ha' }],
      },
    });
  }

  it('edits a Director night, previews the draft and saves it', async () => {
    const director = directorRig();
    mount(plans([director]));
    const requests: Array<{ override_draft?: { entries: Array<{ kind: string; subject?: unknown }> } }> = [];
    const saves: Array<{ expected_revision: number; entries: unknown[] }> = [];
    server.use(
      http.post('/api/director/v1/night-plans', async ({ request }) => {
        requests.push(await request.json() as (typeof requests)[number]);
        return HttpResponse.json(ok(plans([director])));
      }),
      http.get('/api/director/v1/rigs/:rig/night-overrides/:date', ({ params }) =>
        HttpResponse.json(ok({ date: params.date, revision: 3, entries: [], runtime: 'director' }))),
      http.put('/api/director/v1/rigs/:rig/night-overrides/:date', async ({ request, params }) => {
        const body = await request.json() as (typeof saves)[number];
        saves.push(body);
        return HttpResponse.json(ok({ date: params.date, revision: 4, entries: body.entries, runtime: 'director' }));
      }),
    );
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    const overrides = within(detail).getByRole('region', { name: 'Overrides' });
    expect(await within(overrides).findByText('None: the rig follows its own program this night.')).toBeInTheDocument();
    fireEvent.click(within(overrides).getByRole('button', { name: 'Edit night' }));
    expect(within(overrides).getByText(/Times are in the site's time zone \(UTC\)/)).toBeInTheDocument();
    fireEvent.change(within(overrides).getByLabelText('From'), { target: { value: '21:00' } });
    fireEvent.change(within(overrides).getByLabelText('To'), { target: { value: '23:00' } });
    fireEvent.click(within(overrides).getByRole('button', { name: 'Add' }));
    expect(within(overrides).getByRole('listitem')).toHaveTextContent(/^Only IC 1805 from /);
    // The schedule is planned again with the edit, unsaved.
    await waitFor(() => expect(requests.some(r => r.override_draft?.entries[0]?.kind === 'pin')).toBe(true));
    const drafted = requests.find(r => r.override_draft)!.override_draft!;
    expect(drafted.entries[0].subject).toEqual({ kind: 'target', target_guid: 't1' });
    fireEvent.click(within(overrides).getByRole('button', { name: 'Save overrides' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].expected_revision).toBe(3);
    expect(saves[0].entries).toEqual(drafted.entries);
    expect(await within(overrides).findByRole('button', { name: 'Edit night' })).toBeInTheDocument();
  });

  it('says in the grid which nights have overrides', async () => {
    const director = directorRig();
    mount(plans([{ ...director, nights: [night(0, { override_count: 2 }), night(1), night(2)] }]));
    const grid = await screen.findByTestId('nights-grid');
    const cells = within(grid).getAllByRole('button');
    expect(cells[0]).toHaveAccessibleName(/, 2 overrides$/);
    expect(cells[1]).not.toHaveAccessibleName(/override/);
  });

  it('shows no overrides for a Target Scheduler rig', async () => {
    mount();
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).queryByRole('region', { name: 'Overrides' })).toBeNull();
  });
});
