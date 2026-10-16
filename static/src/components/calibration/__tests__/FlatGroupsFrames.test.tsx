import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { createMemoryRouter, RouterProvider, useLocation } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import type { CalibrationSession, FlatGroupsRig, FlatGroupsView, FlatSession } from '../../../api/types';
import { server } from '../../../test/msw-server';
import { OPEN_SETTINGS_EVENT, type OpenSettingsDetail } from '../../../utils/settingsIntent';
import { AccessContext, type AccessContextValue } from '../../../auth/access';
import CalibrationPage from '../CalibrationPage';
import { FLAT_POLL_MS } from '../useFlatGroups';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

/** 14:00 UTC on a night of September 2026, in Unix seconds. */
const at = (night: string, minutes = 0) => Date.UTC(2026, 8, Number(night.slice(3)), 14, minutes) / 1000;

function response(key: string, night: string, sessions: string[], previous: FlatSession['previous'] = null): FlatSession {
  const date = night.slice(5);
  return {
    response_key: key, night, first: at(date), last: at(date, 2), frames: 10, combined: 10, noise_pct: 0.05,
    // A binned, non-square grid: 780 × 520 at bin 8 is a 6240 × 4160 sensor.
    bin: 8, width: 780, height: 520, previous, sessions,
  };
}

const spots = [
  { kind: 'new' as const, darker: true, x: 1418, y: 3540, radius: 40, peak_pct: 1.6 },
  { kind: 'gone' as const, darker: false, x: 3000, y: 1200, radius: 30, peak_pct: 0.4 },
];

const RIG: FlatGroupsRig = {
  rig_uuid: 'rig', name: 'UltraCat · ASI6200', measured: true,
  dust_limit_pct: 1, pattern_limit_pct: 2, own_limits: [],
  angle_term: { dust_pct: 0.3, broad_pct: 0.7 }, angle_pairs: 12,
  // The rig computer keeps Central Daylight Time.
  clock: [{ from: 0, offset_seconds: -5 * 3600 }],
  events: [{
    key: 'e1', start: at('09-16', 2), end: at('09-20'), what: 'new dust: 2 shadows near (1418, 3540)',
    dust_pct: 1.6, broad_pct: 0.2, camera_side: true, filters: ['R', 'G'], rotations: [3, 54],
    spots, before_response: 'r-0916', after_response: 'r-0920',
  }, {
    key: 'e2', start: at('09-10', 2), end: at('09-13'), what: 'the light pattern changed',
    dust_pct: 0.1, broad_pct: 2.6, camera_side: false, filters: ['G'], rotations: [3],
    spots: [], before_response: 'g-0910', after_response: 'g-0913',
  }],
  families: [{
    key: 'R:3', filter: 'R', rotation: 3,
    groups: [
      { key: 's-r0920', first_night: '2026-09-20', last_night: '2026-09-20', frames: 10, sessions: [response('r-0920', '2026-09-20', ['s-r0920'])] },
      {
        key: 's-r0907', first_night: '2026-09-07', last_night: '2026-09-16', frames: 20,
        sessions: [
          response('r-0916', '2026-09-16', ['s-r0916'], { dust_pct: 0.09, broad_pct: 0.01 }),
          response('r-0907', '2026-09-07', ['s-r0907']),
        ],
      },
    ],
    splits: [{
      before_response: 'r-0916', after_response: 'r-0920', before_night: '2026-09-16', after_night: '2026-09-20',
      start: at('09-16', 2), end: at('09-20'), dust_pct: 1.6, broad_pct: 0.2,
      reason: 'new dust: 1 shadow near (1418, 3540), 1.6% of the background (limit 1%)', event_key: 'e1', spots,
    }],
  }, {
    key: 'G:3', filter: 'G', rotation: 3,
    groups: [
      { key: 's-g0913', first_night: '2026-09-13', last_night: '2026-09-13', frames: 10, sessions: [response('g-0913', '2026-09-13', ['s-g0913'])] },
      { key: 's-g0910', first_night: '2026-09-10', last_night: '2026-09-10', frames: 10, sessions: [response('g-0910', '2026-09-10', ['s-g0910'])] },
    ],
    splits: [{
      before_response: 'g-0910', after_response: 'g-0913', before_night: '2026-09-10', after_night: '2026-09-13',
      start: at('09-10', 2), end: at('09-13'), dust_pct: 0.1, broad_pct: 2.6,
      reason: 'the light pattern changed by 2.6% (limit 2%)', event_key: 'e2', spots: [],
    }],
  }],
  unmeasured: [{ night: '2026-09-14', filter: 'R', rotation: 3, frames: 2, reason: 'needs 3 flats', sessions: ['s-r0914'] }],
};

const VIEW: FlatGroupsView = { available: true, rigs: [RIG] };

function listed(id: string, night: string, filter: string, overrides: Partial<CalibrationSession> = {}): CalibrationSession {
  return {
    id, kind: 'flat', night: `2026-${night}`, rig_uuid: 'rig', rig_name: 'UltraCat · ASI6200', is_master: false,
    filter, exposure_min_s: 2, exposure_max_s: 2, temp_min_c: -10, temp_max_c: -10, gain: 100, offset: 20,
    readout: null, binning: '1x1', width: 6240, height: 4160, rotation: 3, first_at: at(night), last_at: at(night, 2),
    frames: 10, ok: 10, suspect: 0, poor: 0, ungraded: 0, kept_out: 0, ...overrides,
  };
}

/** Newest night first, as the server lists them. */
const SESSIONS = [
  listed('s-r0920', '09-20', 'R'),
  listed('s-r0916', '09-16', 'R'),
  listed('s-r0914', '09-14', 'R', { frames: 2, ok: 2 }),
  listed('s-g0913', '09-13', 'G'),
  listed('s-g0910', '09-10', 'G'),
  listed('s-r0907', '09-07', 'R'),
  listed('s-d0907', '09-07', '', { kind: 'dark', filter: null, rotation: null, exposure_min_s: 300, exposure_max_s: 300 }),
];

interface Seen {
  groups: number;
  /** Polls of the flat measuring job. */
  quality: number;
  /** The rig each poll said the page shows. */
  watch: (string | null)[];
  /** Resume requests. */
  resumed: number;
  sessions: URLSearchParams[];
}

const IDLE_JOB = {
  state: 'idle', active: false, rig_uuid: null, rig_name: null, stage: null, done: 0, total: 0, rigs: [],
  stopped: [], retry_at: null, priority: null, waiting_for: null, workers: 0, started_at: null, finished_at: null,
  last: null, last_error: null,
};

function serve(view: FlatGroupsView = VIEW, progress: Record<string, unknown> = {}): Seen {
  const seen: Seen = { groups: 0, quality: 0, watch: [], resumed: 0, sessions: [] };
  server.use(
    http.get('/api/info', () => ok({ version: 'test', cache_directory: '/cache', allow_database_management: true })),
    http.get('/api/databases', () => ok([
      { id: 'demo', name: 'Demo', database_path: '/demo.sqlite', image_directories: [], remote_image_upload: { enabled: false } },
    ])),
    http.get('/api/db/demo/calibrations', () => ok({
      schema_version: 9, frame_count: 70, master_count: 0,
      rigs: [{ rig_uuid: 'rig', name: 'UltraCat · ASI6200', frame_count: 70, bias: 0, dark: 10, dark_flat: 0, flat: 60 }],
    })),
    http.get('/api/db/demo/calibrations/sessions', ({ request }) => {
      const params = new URL(request.url).searchParams;
      seen.sessions.push(params);
      const kind = params.get('kind');
      const sessions = SESSIONS.filter((entry) => !kind || entry.kind === kind);
      return ok({
        sessions, total: sessions.length, page: 0, per_page: 50,
        frames: sessions.reduce((sum, entry) => sum + entry.frames, 0),
        facets: { filters: ['G', 'R'], exposures: [2, 300], rotations: [3, 54] },
        night_boundary_seconds: 43200,
      });
    }),
    http.get('/api/db/demo/calibrations/frames', () => ok({ frames: [], total: 0, page: 0, per_page: 500 })),
    http.get('/api/db/demo/calibrations/flat-groups', () => {
      seen.groups += 1;
      return ok(view);
    }),
    http.get('/api/db/demo/calibrations/flat-measure', () => {
      seen.quality += 1;
      return ok({ ...IDLE_JOB, ...progress });
    }),
    http.post('/api/db/demo/calibrations/flat-measure/watch', async ({ request }) => {
      seen.watch.push(((await request.json()) as { rig: string }).rig);
      return ok({ ...IDLE_JOB, ...progress });
    }),
    http.post('/api/db/demo/calibrations/flat-measure/resume', () => {
      seen.resumed += 1;
      return ok({ ...IDLE_JOB });
    }),
    http.post('/api/db/demo/calibrations/generation-status', () => ok({ statuses: [] })),
  );
  return seen;
}

function Location() {
  const location = useLocation();
  return <output aria-label="location">{location.search}</output>;
}

function renderPage(entry: string, access?: Partial<AccessContextValue>) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  // The app's kind of router: a data router.
  const router = createMemoryRouter(
    [{ path: '/calibration', element: <><CalibrationPage /><Location /></> }],
    { initialEntries: [entry] },
  );
  return render(
    <QueryClientProvider client={client}>
      <AccessContext.Provider value={{
        status: { authentication_required: true, authenticated: true, role: 'read_write', can_compute: true },
        canWrite: true, canCompute: true, logout: async () => undefined, ...access,
      }}>
        <RouterProvider router={router} />
      </AccessContext.Provider>
    </QueryClientProvider>,
  );
}

const params = () => new URLSearchParams(screen.getByRole('status', { name: 'location' }).textContent ?? '');

/** The session list's rows in order: a session by its id, a mark by its test id. */
async function rows() {
  const list = within(await screen.findByRole('navigation', { name: 'Sessions' })).getByRole('list');
  await waitFor(() => expect(list.querySelector('[data-testid="flat-group-badge"]')).not.toBeNull());
  return [...list.children].map((row) =>
    row.querySelector('[data-session-id]')?.getAttribute('data-session-id') ?? row.getAttribute('data-testid'));
}


describe('Flat groups in Frames', () => {
  it('has no tab of its own', async () => {
    serve();
    renderPage('/calibration?db=demo');
    await screen.findByRole('tab', { name: 'Frames' });
    expect(screen.queryByRole('tab', { name: 'Flat groups' })).toBeNull();
  });

  it('marks each flat session with its group, and draws the rig-wide change once and a filter\'s split', async () => {
    serve();
    renderPage('/calibration?db=demo&calKind=flat');
    expect(await rows()).toEqual([
      's-r0920',
      // Seen in R and G at two angles: the camera side, once for the rig.
      'flat-change-event',
      's-r0916',
      's-r0914',
      's-g0913',
      // G's vignette alone: a split in G.
      'flat-split',
      's-g0910',
      's-r0907',
    ]);
    const badge = (id: string) =>
      document.querySelector(`[data-session-id="${id}"] [data-testid="flat-group-badge"]`)?.textContent;
    expect(badge('s-r0920')).toBe('Group 2 · 09-20, 1 night');
    expect(badge('s-r0916')).toBe('Group 1 · 09-07 → 09-16, 2 nights');
    expect(badge('s-r0914')).toBe('Not measured: needs 3 flats');
    expect(badge('s-g0913')).toBe('Group 2 · 09-13, 1 night');
    // Neighbouring groups take neighbouring colours.
    const tone = (id: string) => document.querySelector(`[data-session-id="${id}"]`)?.closest('li')?.getAttribute('data-flat-tone');
    expect([tone('s-r0920'), tone('s-r0916'), tone('s-r0907')]).toEqual(['2', '1', '1']);

    const event = screen.getByTestId('flat-change-event');
    expect(event).toHaveAccessibleName('Change in UltraCat · ASI6200: new dust: 2 shadows near (1418, 3540)');
    expect(event).toHaveTextContent('Change · All filters and angles: camera side');
    expect(event).toHaveTextContent('dust 1.6% · pattern 0.20% · seen in R, G at 3°, 54°');
    // On the rig's clock: 14:00 UTC is 09:00 there.
    expect(event).toHaveTextContent('2026-09-16 09:02 – 2026-09-20 09:00 UTC−5');
    expect(within(event).queryByTestId('flat-change-within-run')).toBeNull();
    const split = screen.getByTestId('flat-split');
    expect(split).toHaveAccessibleName('Split in G · 3°');
    expect(split).toHaveTextContent('the light pattern changed by 2.6% (limit 2%)');
    expect(within(split).getByRole('button', { name: 'Compare 2026-09-13 with 2026-09-10' }))
      .toHaveAttribute('aria-pressed', 'false');
  });

  it('says when a change\'s window is narrower than one flat run', async () => {
    const narrow = { ...RIG.events[0], start: at('09-20'), end: at('09-20') + 300, within_flat_run: true };
    serve({ available: true, rigs: [{ ...RIG, events: [narrow, RIG.events[1]] }] });
    renderPage('/calibration?db=demo&calKind=flat');
    await rows();
    expect(within(screen.getByTestId('flat-change-event')).getByTestId('flat-change-within-run'))
      .toHaveTextContent('Narrower than one flat run');
  });

  it('sums up each rig and its limits, with a link to edit them', async () => {
    serve();
    renderPage('/calibration?db=demo&calKind=flat');
    const summary = await screen.findByTestId('flat-rig-summary');
    expect(summary).toHaveTextContent(
      'Splits at 1% dust, 2% light pattern · angle term 0.30% dust, 0.70% pattern (12 pairs) · ' +
      '1 session with too few flats to measure · Edit limits',
    );
  });

  it('shows the chosen session\'s group beside its title', async () => {
    serve();
    renderPage('/calibration?db=demo&calKind=flat&calSession=s-r0916');
    const line = await screen.findByTestId('flat-session-group');
    expect(line).toHaveTextContent(
      'Group 1 of 2 in R · 3° · 2026-09-07 – 2026-09-16 · 2 nights · 20 flats · dust 0.09%, pattern 0.01% against the night before',
    );
  });

  it('opens a change in the ratio picture beside the list, with its spots on the sensor\'s scale', async () => {
    serve();
    renderPage('/calibration?db=demo&calKind=flat');
    const event = await screen.findByTestId('flat-change-event');
    const compare = within(event).getByRole('button', { name: 'Compare 2026-09-20 with 2026-09-16' });
    compare.focus();
    fireEvent.click(compare);
    await waitFor(() => expect(params().get('calEarlier')).toBe('r-0916'));
    expect(params().get('calLater')).toBe('r-0920');
    expect(compare).toHaveAttribute('aria-pressed', 'true');

    const viewer = await screen.findByRole('region', { name: 'Flat comparison' });
    // Focus moves to the comparison, so the keyboard carries on there.
    await waitFor(() => expect(within(viewer).getByRole('heading')).toHaveFocus());
    expect(viewer.closest('.calibration-main')).not.toBeNull();
    expect(within(viewer).getByRole('heading', { name: 'new dust: 2 shadows near (1418, 3540)' })).toBeInTheDocument();
    expect(within(viewer).getByText('2026-09-20 ÷ 2026-09-16', { exact: false })).toBeInTheDocument();
    const image = within(viewer).getByTestId('flat-ratio-image') as HTMLImageElement;
    const src = new URL(image.src);
    expect(src.pathname).toBe('/api/db/demo/calibrations/flat-groups/ratio');
    expect(Object.fromEntries(src.searchParams)).toEqual({ earlier: 'r-0916', later: 'r-0920', stretch: 'normal' });

    const stretch = within(viewer).getByRole('group', { name: 'Stretch' });
    fireEvent.click(within(stretch).getByRole('button', { name: 'Tight' }));
    await waitFor(() => expect(params().get('calStretch')).toBe('tight'));
    expect(within(stretch).getByRole('button', { name: 'Tight' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(stretch).getByRole('button', { name: 'Normal' })).toHaveAttribute('aria-pressed', 'false');

    fireEvent.load(within(viewer).getByTestId('flat-ratio-image'));
    const overlay = await within(viewer).findByTestId('flat-ratio-spots');
    expect(overlay).toHaveAttribute('viewBox', '0 0 6240 4160');
    // Each mark can be tapped or focused for its numbers.
    expect(overlay).toHaveAttribute('aria-label', '2 dust marks');
    expect(within(overlay).getAllByRole('button')).toHaveLength(2);
    // What the circles mark is said in words too.
    const listed = within(viewer).getByRole('list', { name: 'Spots that changed' });
    expect(within(listed).getAllByRole('listitem').map((item) => item.textContent)).toEqual([
      'New: darker by 1.6% near (1418, 3540)',
      'Gone: brighter by 0.40% near (3000, 1200)',
    ]);

    // Close hands focus back to the Compare that opened it.
    fireEvent.click(within(viewer).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(params().get('calEarlier')).toBeNull());
    expect(screen.queryByRole('region', { name: 'Flat comparison' })).toBeNull();
    await waitFor(() => expect(compare).toHaveFocus());
    expect(compare).toHaveAttribute('aria-pressed', 'false');

    // Compare twice opens and closes it.
    fireEvent.click(compare);
    await screen.findByRole('region', { name: 'Flat comparison' });
    fireEvent.click(compare);
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Flat comparison' })).toBeNull());
  });

  it('draws disks, donuts and vignetting that changed at their shape, by what happened, under Spots', async () => {
    const changed = [
      { kind: 'new' as const, darker: true, x: 3080, y: 2970, radius: 295, peak_pct: 0.33, shape: 'disk' as const,
        bounds: [2785, 2675, 3375, 3265] as [number, number, number, number], within_tolerance: false },
      { kind: 'moved' as const, darker: true, x: 520, y: 2060, radius: 120, peak_pct: 0.3, shape: 'donut' as const,
        hole: 0.12, within_tolerance: true, offset_px: 18 },
      { kind: 'gone' as const, darker: false, x: 3000, y: 1200, radius: 30, peak_pct: 0.4 },
    ];
    const vignetting = [{ edge: 'bottom_left' as const, darker: true, x: 170, y: 3940, area: 180000, depth_pct: 2.39 }];
    const families = RIG.families.map((family) => ({
      ...family,
      splits: family.splits.map((split) => ({ ...split, spots: changed, vignetting })),
    }));
    serve({ available: true, rigs: [{ ...RIG, events: [], families }] });
    renderPage('/calibration?db=demo&calKind=flat&calEarlier=r-0916&calLater=r-0920');
    const viewer = await screen.findByRole('region', { name: 'Flat comparison' });
    fireEvent.load(within(viewer).getByTestId('flat-ratio-image'));
    const overlay = await within(viewer).findByTestId('flat-ratio-spots');
    const marks = within(overlay).getAllByRole('button');
    expect(marks.map((mark) => mark.getAttribute('data-shape')).sort()).toEqual(['compact', 'disk', 'donut', 'vignetting']);
    // New, gone and moved each have their own look; a move within focus
    // travel is drawn but not counted.
    expect(marks.find((mark) => mark.getAttribute('data-shape') === 'disk')).toHaveClass('is-new');
    expect(marks.find((mark) => mark.getAttribute('data-shape') === 'donut')).toHaveClass('is-tolerance');
    expect(marks.find((mark) => mark.getAttribute('data-shape') === 'compact')).toHaveClass('is-gone');
    const listed = within(viewer).getByRole('list', { name: 'Spots that changed' });
    expect(within(listed).getAllByRole('listitem').map((item) => item.textContent)).toEqual([
      'New disk: darker by 0.33% near (3080, 2970)',
      'Moved donut within focus travel: darker by 0.30% near (520, 2060)',
      'Gone: brighter by 0.40% near (3000, 1200)',
      'vignetting deeper 2.39% · bottom-left corner: the flats differ, not dust',
    ]);

    // Large dust is a toggle under Spots: off, only the compact spot stays.
    const spotsGroup = within(viewer).getByRole('group', { name: 'Spots' });
    const dust = within(spotsGroup).getByRole('button', { name: 'Large dust' });
    expect(dust).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(dust);
    await waitFor(() => expect(params().get('calDust')).toBe('0'));
    await waitFor(() => expect(within(overlay).getAllByRole('button')).toHaveLength(1));
    // Spots off hides the overlay, and with it the toggle under it.
    fireEvent.click(within(spotsGroup).getByRole('button', { name: 'Spots' }));
    await waitFor(() => expect(params().get('calRatioSpots')).toBe('0'));
    expect(within(viewer).queryByTestId('flat-ratio-spots')).toBeNull();
    expect(within(spotsGroup).queryByRole('button', { name: 'Large dust' })).toBeNull();
  });

  it('draws a picture still being made through the shared poll', async () => {
    serve();
    const asked: unknown[] = [];
    let ready = false;
    server.use(http.post('/api/db/demo/calibrations/generation-status', async ({ request }) => {
      asked.push(await request.json());
      return ok({ statuses: [{ state: ready ? 'ready' : 'generating' }] });
    }));
    renderPage('/calibration?db=demo&calKind=flat&calEarlier=r-0916&calLater=r-0920');
    const viewer = await screen.findByRole('region', { name: 'Flat comparison' });
    const image = within(viewer).getByTestId('flat-ratio-image') as HTMLImageElement;
    const first = image.src;
    // The server answers 202 while it draws: the image fails to load.
    fireEvent.error(image);
    expect(await within(viewer).findByText('Drawing…')).toBeInTheDocument();
    await waitFor(() => expect(asked.length).toBeGreaterThan(0));
    expect(asked[0]).toEqual({ requests: [{ target: 'ratio', earlier: 'r-0916', later: 'r-0920', stretch: 'normal' }] });
    ready = true;
    // Ready: the picture is asked for again, past the browser's cache.
    await waitFor(() => expect((within(viewer).getByTestId('flat-ratio-image') as HTMLImageElement).src).not.toBe(first),
      { timeout: 4000 });
  });

  it('names a pair it cannot find plainly, with no spots, and sizes a picture it knows nothing of by itself', async () => {
    serve({
      available: true,
      rigs: [{ ...RIG, families: RIG.families.map((family) => ({
        ...family,
        groups: family.groups.map((group) => ({
          ...group,
          sessions: group.sessions.map((entry) => ({ ...entry, bin: null, width: null, height: null })),
        })),
      })) }],
    });
    const first = renderPage('/calibration?db=demo&calKind=flat&calEarlier=gone-a&calLater=gone-b');
    const stale = await screen.findByRole('region', { name: 'Flat comparison' });
    expect(within(stale).getByRole('heading', { name: 'Two flat sessions' })).toBeInTheDocument();
    expect(within(stale).getByText(/^Later ÷ earlier/)).toBeInTheDocument();
    expect(within(stale).queryByRole('list', { name: 'Spots that changed' })).toBeNull();
    first.unmount();

    // A session measured without its grid: the picture keeps its own shape,
    // and no circles are offered, though the spots are listed.
    renderPage('/calibration?db=demo&calKind=flat&calEarlier=r-0916&calLater=r-0920');
    const viewer = await screen.findByRole('region', { name: 'Flat comparison' });
    await within(viewer).findByRole('heading', { name: 'new dust: 2 shadows near (1418, 3540)' });
    fireEvent.load(within(viewer).getByTestId('flat-ratio-image'));
    expect(viewer.querySelector('.flat-ratio-canvas')).toHaveClass('is-unsized');
    expect(within(viewer).queryByRole('button', { name: 'Spots' })).toBeNull();
    expect(within(viewer).queryByTestId('flat-ratio-spots')).toBeNull();
    expect(within(viewer).getByRole('list', { name: 'Spots that changed' })).toBeInTheDocument();
  });

  it('sums up each rig of several, and keeps to the rig chosen', async () => {
    const other: FlatGroupsRig = {
      ...RIG, rig_uuid: 'redcat', name: 'RedCat · ASI2600', events: [], families: [], unmeasured: [],
      angle_term: null, angle_pairs: 0, dust_limit_pct: 1.5,
    };
    serve({ available: true, rigs: [RIG, other] });
    const first = renderPage('/calibration?db=demo&calKind=flat');
    await waitFor(() => expect(screen.getAllByTestId('flat-rig-summary')).toHaveLength(2));
    expect(screen.getAllByTestId('flat-rig-summary').map((line) => line.querySelector('strong')?.textContent))
      .toEqual(['UltraCat · ASI6200: ', 'RedCat · ASI2600: ']);
    expect(screen.getAllByTestId('flat-rig-summary')[1])
      .toHaveTextContent('Splits at 1.5% dust, 2% light pattern · no flats measured yet');
    first.unmount();

    serve({ available: true, rigs: [RIG, other] });
    renderPage('/calibration?db=demo&calKind=flat&calRig=redcat');
    const lines = await screen.findAllByTestId('flat-rig-summary');
    expect(lines).toHaveLength(1);
    // One rig in view: its name goes without saying.
    expect(lines[0].querySelector('strong')).toBeNull();
    expect(lines[0]).toHaveTextContent(/^Splits at 1.5% dust/);
  });

  it('shows a rig the address names that the library lacks, as set, and clears it with the rest', async () => {
    const seen = serve();
    renderPage('/calibration?db=demo&calKind=flat&calRig=gone&calFilter=R&calRotation=3');
    const rig = await screen.findByRole('combobox', { name: 'Rig' });
    await waitFor(() => expect(rig).toHaveValue('gone'));
    expect(within(rig).getByRole('option', { name: 'A rig not in this library' })).toBeInTheDocument();
    expect(rig.closest('.toolbar-pill')).toHaveClass('is-set');
    await waitFor(() => expect(seen.sessions.at(-1)?.get('rig')).toBe('gone'));

    fireEvent.click(screen.getAllByRole('button', { name: /^Clear/ })[0]);
    await waitFor(() => expect(params().get('calRig')).toBeNull());
    expect(params().get('calRotation')).toBeNull();
    expect(params().get('calFilter')).toBeNull();
    expect(params().get('calKind')).toBeNull();
    // Cleared to every kind, flat groups still ride along.
    expect(await screen.findByTestId('flat-rig-summary')).toBeInTheDocument();
  });

  it('opens an old Flat groups link as Frames on flats, with its rig, filter, angle and pair', async () => {
    const seen = serve();
    renderPage('/calibration?db=demo&calTab=groups&calRig=rig&calFilter=R&calRotation=3&calEarlier=r-0916&calLater=r-0920&calStretch=wide');
    await waitFor(() => expect(params().get('calTab')).toBeNull());
    expect(Object.fromEntries(params())).toEqual({
      db: 'demo', calKind: 'flat', calRig: 'rig', calFilter: 'R', calRotation: '3',
      calEarlier: 'r-0916', calLater: 'r-0920', calStretch: 'wide',
    });
    expect(screen.getByRole('tab', { name: 'Frames' })).toHaveAttribute('aria-selected', 'true');
    expect(await screen.findByRole('region', { name: 'Flat comparison' })).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Angle' })).toHaveValue('3');
    await waitFor(() => expect(seen.sessions.at(-1)?.get('rotation')).toBe('3'));
    expect(seen.sessions.at(-1)?.get('kind')).toBe('flat');
    expect(seen.sessions.at(-1)?.get('filter')).toBe('R');
  });

  it('sends Edit limits to the global setting, or to the rig\'s own', async () => {
    const asked: OpenSettingsDetail[] = [];
    const listen = (event: Event) => asked.push((event as CustomEvent<OpenSettingsDetail>).detail);
    window.addEventListener(OPEN_SETTINGS_EVENT, listen);
    try {
      serve();
      const first = renderPage('/calibration?db=demo&calKind=flat');
      fireEvent.click(await screen.findByRole('button', { name: 'Edit limits for UltraCat · ASI6200' }));
      expect(asked.at(-1)).toEqual({ intent: 'calibration', field: 'flat_split_dust_pct' });
      first.unmount();

      serve({ available: true, rigs: [{ ...RIG, own_limits: ['flat_split_pattern_pct'] }] });
      const second = renderPage('/calibration?db=demo&calKind=flat');
      fireEvent.click(await screen.findByRole('button', { name: 'Edit limits for UltraCat · ASI6200' }));
      expect(asked.at(-1)).toEqual({
        intent: 'calibration',
        rig: { uuid: 'rig', name: 'UltraCat · ASI6200' },
        field: 'flat_split_pattern_pct',
      });
      second.unmount();

      // Grouped by time: Group flats is the setting that decides.
      serve({ available: true, rigs: [{ ...RIG, measured: false }] });
      renderPage('/calibration?db=demo&calKind=flat');
      fireEvent.click(await screen.findByRole('button', { name: 'Edit limits for UltraCat · ASI6200' }));
      expect(asked.at(-1)).toEqual({ intent: 'calibration', field: 'flat_grouping' });
    } finally {
      window.removeEventListener(OPEN_SETTINGS_EVENT, listen);
    }
  });

  it('says why flats group by time, and when none are measured yet', async () => {
    serve({ available: false, rigs: [] });
    const first = renderPage('/calibration?db=demo&calKind=flat');
    expect(await screen.findByText('This catalog cannot be written, so flats are grouped by time.')).toBeInTheDocument();
    first.unmount();

    serve({ available: true, rigs: [{ ...RIG, measured: false, events: [], families: [], unmeasured: [] }] });
    const second = renderPage('/calibration?db=demo&calKind=flat');
    expect(await screen.findByTestId('flat-rig-summary')).toHaveTextContent(/^Grouped by time · angle term/);
    second.unmount();

    serve({ available: true, rigs: [{ ...RIG, events: [], families: [], unmeasured: [] }] });
    renderPage('/calibration?db=demo&calKind=flat');
    expect(await screen.findByTestId('flat-rig-summary')).toHaveTextContent('no flats measured yet');
  });

  it('asks nothing about flat groups while the list holds no flats', async () => {
    const seen = serve();
    renderPage('/calibration?db=demo&calKind=dark');
    await waitFor(() => expect(seen.sessions.length).toBeGreaterThan(0));
    expect(seen.groups).toBe(0);
    expect(seen.quality).toBe(0);
    expect(screen.queryByTestId('flat-groups-summary')).toBeNull();
  });

  it('shows the flats being measured with one poll, says each stage once, and reads the groups again when it ends', async () => {
    const seen = serve();
    const poll = { ...FLAT_POLL_MS };
    FLAT_POLL_MS.running = 50;
    try {
      const running = { state: 'running', active: true, rig_uuid: 'rig', rig_name: 'UltraCat · ASI6200', priority: 'background' };
      let progress: Record<string, unknown> = { ...running, stage: 'measuring flat sessions', done: 12, total: 40 };
      server.use(http.get('/api/db/demo/calibrations/flat-measure', () => {
        seen.quality += 1;
        return ok({ ...IDLE_JOB, ...progress });
      }));
      renderPage('/calibration?db=demo&calKind=flat&calSession=s-r0916');
      expect(await screen.findByTestId('flat-groups-measuring')).toHaveTextContent('Measuring flats · 12 of 40 sessions');
      expect(screen.getByTestId('flat-groups-status')).toHaveAttribute('role', 'status');
      expect(screen.getByTestId('flat-groups-status')).toHaveTextContent('Measuring flats: measuring flat sessions');
      await waitFor(() => expect(seen.groups).toBe(1));

      // Later stages name themselves; the live region changes with the
      // stage, not with each count.
      progress = { ...running, stage: 'comparing flat sessions', done: 4, total: 9 };
      await waitFor(() => expect(screen.getByTestId('flat-groups-measuring')).toHaveTextContent('Comparing flats · 4 of 9 pairs'));
      expect(screen.getByTestId('flat-groups-status')).toHaveTextContent('Measuring flats: comparing flat sessions');
      progress = { ...running, stage: 'reading lights shot during a change', done: 1, total: 2, waiting_for: 'a stack build' };
      await waitFor(() => expect(screen.getByTestId('flat-groups-measuring')).toHaveTextContent(
        'Reading lights shot during a change · 1 of 2 sessions · waiting for a stack build'));

      // The pass ends; the next poll finds it done and the groups are read again.
      const polls = seen.quality;
      progress = { state: 'idle', active: false, finished_at: 1760000000 };
      await waitFor(() => expect(screen.queryByTestId('flat-groups-measuring')).toBeNull());
      expect(screen.getByTestId('flat-groups-status')).toHaveTextContent('Flats measured; the groups are up to date.');
      await waitFor(() => expect(seen.groups).toBe(2));
      // The summary, the list and the chosen session share one poll: it
      // ticks once per interval, not once per part.
      expect(seen.quality - polls).toBeLessThanOrEqual(2);
    } finally {
      Object.assign(FLAT_POLL_MS, poll);
    }
  });

  it('tells the server which rig the page shows, and leaves out another rig\'s pass', async () => {
    const other = { state: 'running', active: true, rig_uuid: 'redcat', rig_name: 'RedCat', stage: 'measuring flat sessions', done: 2, total: 5 };
    const seen = serve(VIEW, other);
    renderPage('/calibration?db=demo&calKind=flat&calRig=rig');
    await waitFor(() => expect(seen.groups).toBe(1));
    await waitFor(() => expect(seen.watch).toContain('rig'));
    expect(screen.queryByTestId('flat-groups-measuring')).toBeNull();
  });

  it('sends no hint for a viewer the server keeps from display calculations', async () => {
    const seen = serve(VIEW, { state: 'running', active: true, rig_uuid: 'rig', stage: 'measuring flat sessions', done: 1, total: 4 });
    renderPage('/calibration?db=demo&calKind=flat&calRig=rig', { canCompute: false, canWrite: false });
    expect(await screen.findByTestId('flat-groups-measuring')).toHaveTextContent('Measuring flats · 1 of 4 sessions');
    await waitFor(() => expect(seen.quality).toBeGreaterThan(0));
    expect(seen.watch).toEqual([]);
  });

  it('lifts no rig when every rig is in view, and still shows the pass', async () => {
    const seen = serve(VIEW, {
      state: 'running', active: true, rig_uuid: 'rig', rig_name: 'UltraCat · ASI6200',
      stage: 'measuring flat sessions', done: 12, total: 40, priority: 'background',
    });
    renderPage('/calibration?db=demo&calKind=flat');
    expect(await screen.findByTestId('flat-groups-measuring')).toHaveTextContent('Measuring flats · 12 of 40 sessions');
    expect(screen.getByTestId('flat-groups-measuring')).toHaveAttribute('data-priority', 'background');
    expect(seen.watch).toEqual([]);
  });

  it('says Stop paused measuring, how it comes back, and resumes it on request', async () => {
    const seen = serve(VIEW, { state: 'stopped', stopped: ['rig'] });
    renderPage('/calibration?db=demo&calKind=flat&calRig=rig');
    const note = await screen.findByTestId('flat-measure-stopped');
    expect(note).toHaveTextContent('Flat measuring is stopped. It resumes when these flats change, or when the server starts again.');
    fireEvent.click(screen.getByTestId('flat-measure-resume'));
    await waitFor(() => expect(seen.resumed).toBe(1));
    await waitFor(() => expect(screen.queryByTestId('flat-measure-stopped')).toBeNull());
  });
});
