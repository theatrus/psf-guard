import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../../test/msw-server';
import RigsPage from '../../rigs/RigsPage';
import { NOON, night, plans, rig } from './fixtures';
import { clockOnNight, formatNightDate } from '../nightsModel';

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
  return { ...render(<RigsPage />, { wrapper: Wrapper }), client, planned, saved, writes };
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
    const plansTonight = within(detail).getByRole('region', { name: 'No target can be imaged this night' });
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
    expect(within(detail).getByText('Clear means 30% cloud or less.')).toBeInTheDocument();
    // The provider's credit is under the grid, once for the page.
    expect(screen.getByRole('link', { name: 'Weather data by Open-Meteo.com (CC BY 4.0)' })).toHaveAttribute('href', 'https://open-meteo.com/');
    // The forecast's mark is not one to edit or clear.
    fireEvent.change(within(detail).getByLabelText('For'), { target: { value: 'site' } });
    expect(within(detail).getByLabelText('Not available')).toBeChecked();
    expect(within(detail).queryByRole('button', { name: 'Clear mark' })).toBeNull();
  });

  it('shows when the plans were computed, reads again while newer ones are computed, and asks for fresh ones', async () => {
    const { planned } = mount({ ...plans(), refreshing: true });
    await screen.findByTestId('nights-grid');
    expect(screen.getByText('Updating…')).toBeInTheDocument();
    // The server said newer plans were being computed: the page reads again
    // shortly, without being asked.
    planned.mockImplementation(() => HttpResponse.json(ok({ ...plans(), computed_at_ms: NOON + 60_000 })));
    expect(await screen.findByText(/^Updated /, {}, { timeout: 5000 })).toBeInTheDocument();
    const requests: Array<{ refresh?: boolean }> = [];
    server.use(http.post('/api/director/v1/night-plans', async ({ request }) => {
      requests.push(await request.json() as { refresh?: boolean });
      return HttpResponse.json(ok(plans()));
    }));
    fireEvent.click(screen.getByRole('button', { name: 'Compute again' }));
    await waitFor(() => expect(requests).toEqual([expect.objectContaining({ refresh: true })]));
  }, 10_000);

  it('shows a night clear only in daylight as out, and lists no daytime hours', async () => {
    const HOUR = 3_600_000;
    const base = night(0);
    const daylight = night(0, {
      available_dark_hours: 0,
      availability: { state: 'limited', marks: [], windows: [{ start_ms: base.noon_ms + 2 * HOUR, end_ms: base.noon_ms + 4 * HOUR }] },
    });
    mount(plans([rig({ nights: [daylight, night(1), night(2)] })]));
    const grid = await screen.findByTestId('nights-grid');
    const cells = within(grid).getAllByRole('button');
    expect(cells[0]).toHaveClass('is-unavailable');
    expect(cells[0]).toHaveAccessibleName(/none clear of 10\.0 h dark, not available/);
    fireEvent.click(cells[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText('None after dark')).toBeInTheDocument();
  });

  it('says when the site\'s forecast does not reach a night', async () => {
    mount(plans([rig({ forecast: { provider: 'met_norway', attribution: 'Weather data from MET Norway (CC BY 4.0)', cloud_threshold_percent: 30, fetched_at_ms: null, expires_at_ms: null } })]));
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[2]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText(/^No current forecast from MET Norway, so the night counts as clear\./)).toBeInTheDocument();
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

  /** Mount a Director rig's night 0 with its overrides at `revision`, and
   *  record the night-plan requests and the saves. */
  async function openDirectorNight(options: { revision?: number; zone?: string; save?: (body: { expected_revision: number; entries: unknown[] }) => Response | undefined } = {}) {
    const director = { ...directorRig(), time_zone: options.zone ?? 'UTC' };
    const { client } = mount(plans([director]));
    const state = { revision: options.revision ?? 3, reads: 0 };
    const requests: Array<{ override_draft?: { entries: Array<{ kind: string; subject?: unknown }> } }> = [];
    const saves: Array<{ expected_revision: number; entries: unknown[] }> = [];
    server.use(
      http.post('/api/director/v1/night-plans', async ({ request }) => {
        requests.push(await request.json() as (typeof requests)[number]);
        return HttpResponse.json(ok(plans([director])));
      }),
      http.get('/api/director/v1/rigs/:rig/night-overrides/:date', ({ params }) => {
        state.reads += 1;
        return HttpResponse.json(ok({ date: params.date, revision: state.revision, entries: [], runtime: 'director' }));
      }),
      http.put('/api/director/v1/rigs/:rig/night-overrides/:date', async ({ request, params }) => {
        const body = await request.json() as (typeof saves)[number];
        saves.push(body);
        return options.save?.(body) ?? HttpResponse.json(ok({ date: params.date, revision: state.revision + 1, entries: body.entries, runtime: 'director' }));
      }),
    );
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText('Director decides')).toBeInTheDocument();
    // Editing waits for the night's saved overrides and its goals.
    const edit = await within(detail).findByRole('button', { name: 'Edit night' });
    return { detail, edit, state, requests, saves, client };
  }

  it('edits a Director night on its timeline, plans the draft and saves it from the bar', async () => {
    const { detail, edit, requests, saves } = await openDirectorNight();
    const overrides = within(detail).getByRole('region', { name: 'Overrides' });
    expect(within(overrides).getByText('None.')).toBeInTheDocument();
    const before = requests.length;
    fireEvent.click(edit);
    // An edit that changes nothing yet is not planned again, so a block
    // can be moved at once.
    expect(within(detail).queryByText('Planning the edit...')).toBeNull();
    expect(requests).toHaveLength(before);
    // A block's menu pins its target to the block.
    fireEvent.click(within(detail).getByRole('button', { name: /^Heart Nebula, IC 1805:/ }));
    fireEvent.click(within(detail).getByRole('menuitem', { name: 'Pin it here' }));
    expect(within(overrides).getByRole('listitem')).toHaveTextContent(/^Only IC 1805 from /);
    // The schedule is planned again with the edit, unsaved, and the grid's
    // saved schedule no longer says when a plan is done.
    await waitFor(() => expect(requests.some(r => r.override_draft?.entries[0]?.kind === 'pin')).toBe(true));
    expect(within(detail).queryByRole('columnheader', { name: 'Done by' })).toBeNull();
    const drafted = requests.find(r => r.override_draft?.entries[0]?.kind === 'pin')!.override_draft!;
    expect(drafted.entries[0].subject).toEqual({ kind: 'target', target_guid: 't1' });
    const bar = within(detail).getByRole('region', { name: 'Unsaved changes' });
    expect(bar).toHaveTextContent(/This night's overrides: Only IC 1805 from /);
    fireEvent.click(within(bar).getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].expected_revision).toBe(3);
    expect(saves[0].entries).toEqual(drafted.entries);
    expect(await within(detail).findByRole('button', { name: 'Edit night' })).toBeInTheDocument();
  });

  it('saves against the revision the edit started from, and drops the edit when that is gone', async () => {
    const { detail, edit, state, saves, client } = await openDirectorNight({
      save: () => HttpResponse.json({ success: false, data: null, error: 'This record changed since it was loaded. Reload, then try again.' }, { status: 409 }),
    });
    fireEvent.click(edit);
    // Someone else saves while this edit is open, and the night is read again.
    state.revision = 7;
    await act(() => client.invalidateQueries({ queryKey: ['nightOverrides'] }));
    await waitFor(() => expect(state.reads).toBe(2));
    fireEvent.keyDown(within(detail).getByRole('slider', { name: 'Hold the rig from' }), { key: 'Home' });
    const bar = within(detail).getByRole('region', { name: 'Unsaved changes' });
    fireEvent.click(within(bar).getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0].expected_revision).toBe(3);
    expect(await within(detail).findByText(/changed since the edit started/)).toBeInTheDocument();
    expect(within(detail).getByRole('button', { name: 'Edit night' })).toBeInTheDocument();
    expect(within(detail).queryByRole('slider', { name: 'Hold the rig from' })).toBeNull();
    await waitFor(() => expect(state.reads).toBeGreaterThan(2));
  });

  it('refuses a pin that would share time with another, and keeps the timeline', async () => {
    const { detail, edit } = await openDirectorNight();
    fireEvent.click(edit);
    for (let pinned = 0; pinned < 2; pinned++) {
      fireEvent.click(within(detail).getByRole('button', { name: /^Heart Nebula, IC 1805:/ }));
      fireEvent.click(within(detail).getByRole('menuitem', { name: 'Pin it here' }));
    }
    expect(within(detail).getByRole('alert')).toHaveTextContent('Two pins would share time');
    const overrides = within(detail).getByRole('region', { name: 'Overrides' });
    expect(within(overrides).getAllByRole('listitem')).toHaveLength(1);
    expect(within(detail).getByRole('slider', { name: 'Hold the rig from' })).toBeInTheDocument();
  });

  it('moves a block again after its pin was nudged, in place of what the first move made', async () => {
    const { detail, edit } = await openDirectorNight();
    fireEvent.click(edit);
    const settled = () => waitFor(() => expect(within(detail).queryByText('Planning the edit...')).toBeNull());
    await settled();
    fireEvent.keyDown(within(detail).getByRole('button', { name: /^Heart Nebula, IC 1805:/ }), { key: 'ArrowRight' });
    await settled();
    const row = within(detail).getByRole('group', { name: 'Overrides' });
    fireEvent.keyDown(within(row).getByRole('button', { name: /^Only IC 1805 from/ }), { key: 'ArrowRight' });
    await settled();
    fireEvent.keyDown(within(detail).getByRole('button', { name: /^Heart Nebula, IC 1805:/ }), { key: 'ArrowRight' });
    expect(within(detail).queryByRole('alert')).toBeNull();
    const listed = within(within(detail).getByRole('region', { name: 'Overrides' })).getAllByRole('listitem').map(item => item.textContent ?? '');
    expect(listed.filter(text => text.startsWith('Only IC 1805'))).toHaveLength(1);
  });

  it('asks before closing a night, or choosing another, with its edits unsaved', async () => {
    const { detail, edit } = await openDirectorNight();
    fireEvent.click(edit);
    fireEvent.click(within(detail).getByRole('button', { name: /^Heart Nebula, IC 1805:/ }));
    fireEvent.click(within(detail).getByRole('menuitem', { name: 'Pin it here' }));
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    fireEvent.click(within(detail).getByRole('button', { name: 'Close night details' }));
    fireEvent.click(within(screen.getByTestId('nights-grid')).getAllByRole('button')[1]);
    expect(confirm).toHaveBeenCalledTimes(2);
    expect(confirm).toHaveBeenLastCalledWith("Discard the unsaved changes in This night's overrides?");
    expect(within(screen.getByRole('region', { name: 'Night details' })).getByRole('region', { name: 'Unsaved changes' })).toBeInTheDocument();
    confirm.mockReturnValue(true);
    fireEvent.click(within(detail).getByRole('button', { name: 'Close night details' }));
    expect(screen.queryByRole('region', { name: 'Night details' })).toBeNull();
    confirm.mockRestore();
  });

  it('refuses a typed hold that falls outside the night', async () => {
    // A site fourteen hours east of UTC: some of its clock times fall
    // outside a night that runs from noon to noon in the test's clock.
    const zone = 'Pacific/Kiritimati';
    const { detail, edit } = await openDirectorNight({ zone });
    fireEvent.click(edit);
    const overrides = within(detail).getByRole('region', { name: 'Overrides' });
    fireEvent.click(within(overrides).getByLabelText('Hold', { exact: true }));
    const tonight = night(0);
    const clocks = Array.from({ length: 48 }, (_, half) => `${String(Math.floor(half / 2)).padStart(2, '0')}:${half % 2 ? '30' : '00'}`);
    const value = clocks.find(clock => {
      const at = clockOnNight(tonight.date, clock, zone)!;
      return at < tonight.noon_ms || at >= tonight.noon_ms + 86_400_000;
    })!;
    expect(value).toBeDefined();
    fireEvent.change(within(overrides).getByLabelText('From'), { target: { value } });
    fireEvent.click(within(overrides).getByRole('button', { name: 'Add' }));
    expect(within(overrides).getByRole('alert')).toHaveTextContent('falls outside this night');
    expect(within(overrides).queryByRole('listitem')).toBeNull();
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

describe('Target Scheduler settings', () => {
  const tsRig = () => rig({
    ts_projects: [
      { project_id: 7, name: 'Heart Nebula', state: 'active', priority: 'normal', targets: [{ target_id: 70, name: 'IC 1805', active: true }] },
      { project_id: 8, name: 'Soul Nebula', state: 'inactive', priority: 'low', targets: [{ target_id: 80, name: 'IC 1848', active: true }] },
    ],
  });

  it('plans a Target Scheduler rig with changed settings, and saves them with what they were', async () => {
    const ts = tsRig();
    mount(plans([ts]));
    const requests: Array<{ ts_draft?: { changes: { projects: unknown[]; targets: unknown[] } } }> = [];
    const saves: unknown[] = [];
    server.use(
      http.post('/api/director/v1/night-plans', async ({ request }) => {
        requests.push(await request.json() as (typeof requests)[number]);
        return HttpResponse.json(ok(plans([ts])));
      }),
      http.put('/api/director/v1/rigs/:rig/target-scheduler', async ({ request }) => {
        saves.push(await request.json());
        return HttpResponse.json(ok({ files: ['/data/redcat.sqlite'], projects: 1, targets: 1 }));
      }),
    );
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    expect(within(detail).getByText('Target Scheduler decides')).toBeInTheDocument();
    // Folded until asked for, with how many projects are active.
    const settings = within(detail).getByRole('region', { name: 'Target Scheduler settings' });
    expect(settings).toHaveTextContent('1 of 2 projects active');
    expect(within(settings).queryByLabelText('Heart Nebula active')).toBeNull();
    // Only the rig's own runtime plans the night.
    expect(within(detail).queryByRole('button', { name: /^Compare with/ })).toBeNull();
    expect(within(detail).queryByLabelText("As Target Scheduler's preview")).toBeNull();
    fireEvent.click(within(settings).getByRole('button', { name: 'Target Scheduler settings' }));
    expect(within(settings).getByRole('button', { name: 'Target Scheduler settings' })).toHaveAttribute('aria-expanded', 'true');
    expect(settings).toHaveTextContent('Changes apply to every night, not just this one.');
    expect(within(settings).getByLabelText('Heart Nebula active')).toBeDisabled();
    fireEvent.click(within(settings).getByRole('button', { name: 'Change settings' }));
    fireEvent.click(within(settings).getByLabelText('Heart Nebula active'));
    fireEvent.change(within(settings).getByLabelText('Soul Nebula priority'), { target: { value: 'high' } });
    fireEvent.click(within(settings).getByLabelText('Soul Nebula active'));
    fireEvent.click(within(settings).getByLabelText('Heart Nebula · IC 1805 active'));
    await waitFor(() => expect(requests.some(r => r.ts_draft?.changes.projects.length === 2)).toBe(true));
    const bar = within(detail).getByRole('region', { name: 'Unsaved changes' });
    expect(bar).toHaveTextContent('Target Scheduler settings: Heart Nebula: inactive; Soul Nebula: active, priority High; Heart Nebula · IC 1805: not active');
    fireEvent.click(within(bar).getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    expect(saves[0]).toEqual({
      projects: [
        { project_id: 7, state: 'inactive', was: { priority: 'normal', state: 'active' } },
        { project_id: 8, priority: 'high', state: 'active', was: { priority: 'low', state: 'inactive' } },
      ],
      targets: [{ target_id: 70, active: false, was_active: true }],
    });
    expect(await within(settings).findByRole('button', { name: 'Change settings' })).toBeInTheDocument();
  });

  it('saves what each row held when the edit started, though the night is read again', async () => {
    const ts = tsRig();
    let served = ts;
    const { client } = mount(plans([ts]));
    const saves: Array<{ projects: Array<{ was: unknown }> }> = [];
    server.use(
      http.post('/api/director/v1/night-plans', () => HttpResponse.json(ok(plans([served])))),
      http.put('/api/director/v1/rigs/:rig/target-scheduler', async ({ request }) => {
        saves.push(await request.json() as (typeof saves)[number]);
        return HttpResponse.json(ok({ files: ['/data/redcat.sqlite'], projects: 1, targets: 0 }));
      }),
    );
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    const settings = within(detail).getByRole('region', { name: 'Target Scheduler settings' });
    fireEvent.click(within(settings).getByRole('button', { name: 'Target Scheduler settings' }));
    fireEvent.click(within(settings).getByRole('button', { name: 'Change settings' }));
    fireEvent.change(within(settings).getByLabelText('Heart Nebula priority'), { target: { value: 'high' } });
    // Someone sets Heart Nebula low in Target Scheduler, and the grid reads
    // the rig again.
    served = { ...ts, catalog_name: 'RedCat rig, read again', ts_projects: ts.ts_projects!.map(project => project.project_id === 7 ? { ...project, priority: 'low' as const } : project) };
    await act(() => client.invalidateQueries({ queryKey: ['nightPlans'] }));
    expect(await within(detail).findByRole('heading', { name: /^RedCat rig, read again/ })).toBeInTheDocument();
    expect(within(settings).getByLabelText('Heart Nebula priority')).toHaveValue('high');
    fireEvent.click(within(within(detail).getByRole('region', { name: 'Unsaved changes' })).getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(saves).toHaveLength(1));
    // What the edit read, so the server refuses the row rather than write
    // over the other change.
    expect(saves[0].projects).toEqual([{ project_id: 7, priority: 'high', was: { priority: 'normal', state: 'active' } }]);
  });

  it('drops the edit when a row changed since it was read', async () => {
    const ts = tsRig();
    mount(plans([ts]));
    server.use(http.put('/api/director/v1/rigs/:rig/target-scheduler', () =>
      HttpResponse.json({ success: false, data: null, error: 'Heart Nebula changed in the rig\'s database since the night was read, so nothing was saved. Read the night again.' }, { status: 409 })));
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const detail = screen.getByRole('region', { name: 'Night details' });
    const settings = within(detail).getByRole('region', { name: 'Target Scheduler settings' });
    fireEvent.click(within(settings).getByRole('button', { name: 'Target Scheduler settings' }));
    fireEvent.click(within(settings).getByRole('button', { name: 'Change settings' }));
    fireEvent.change(within(settings).getByLabelText('Heart Nebula priority'), { target: { value: 'high' } });
    fireEvent.click(within(within(detail).getByRole('region', { name: 'Unsaved changes' })).getByRole('button', { name: 'Save changes' }));
    expect(await within(settings).findByRole('alert')).toHaveTextContent('Heart Nebula changed in the rig\'s database');
    expect(within(settings).getByRole('button', { name: 'Change settings' })).toBeInTheDocument();
  });

  it('says why a rig on another PSF Guard cannot be changed here', async () => {
    mount(plans([{ ...tsRig(), ts_save: { files: [], refused: 'This rig runs on another PSF Guard (observatory): change its Target Scheduler settings there.' } }]));
    const grid = await screen.findByTestId('nights-grid');
    fireEvent.click(within(grid).getAllByRole('button')[0]);
    const settings = within(screen.getByRole('region', { name: 'Night details' })).getByRole('region', { name: 'Target Scheduler settings' });
    fireEvent.click(within(settings).getByRole('button', { name: 'Target Scheduler settings' }));
    expect(settings).toHaveTextContent('runs on another PSF Guard (observatory)');
    expect(within(settings).queryByRole('button', { name: 'Change settings' })).toBeNull();
  });
});
