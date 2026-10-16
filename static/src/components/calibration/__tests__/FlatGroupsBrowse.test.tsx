import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { configure, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { CalibrationSession, FlatGroupsRig, FlatGroupUseView, FlatSession } from '../../../api/types';
import { server } from '../../../test/msw-server';
import { OPEN_SETTINGS_EVENT, type OpenSettingsDetail } from '../../../utils/settingsIntent';
import CalibrationPage from '../CalibrationPage';
import { browseGroups, groupName } from '../flatGroupBrowse';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

/** 14:00 UTC on a night of September 2026, in Unix seconds. */
const at = (night: string, minutes = 0) => Date.UTC(2026, 8, Number(night.slice(3)), 14, minutes) / 1000;

function response(key: string, night: string, sessions: string[], frames = 10): FlatSession {
  const date = night.slice(5);
  return {
    response_key: key, night, first: at(date), last: at(date, 2), frames, combined: frames, noise_pct: 0.05,
    bin: 8, width: 780, height: 520, previous: null, sessions,
  };
}

const spots = [{ kind: 'new' as const, darker: true, x: 1418, y: 3540, radius: 40, peak_pct: 1.6 }];

/** R: group 1 (09-07, 09-16) and group 2 (09-20), split at new dust seen
 *  in R and G; R 09-14 has too few flats to measure. G: 09-10 and 09-13,
 *  split at a changed vignette. */
const RIG: FlatGroupsRig = {
  rig_uuid: 'rig', name: 'UltraCat', measured: true,
  dust_limit_pct: 1, pattern_limit_pct: 2, own_limits: [],
  angle_term: null, angle_pairs: 0,
  events: [{
    key: 'e1', start: at('09-16', 2), end: at('09-20'), what: 'new dust: 1 shadow near (1418, 3540)',
    dust_pct: 1.6, broad_pct: 0.2, camera_side: true, filters: ['R', 'G'], rotations: [3],
    spots, before_response: 'r-0916', after_response: 'r-0920',
  }, {
    key: 'e2', start: at('09-10', 2), end: at('09-13'), what: 'the light pattern changed by 2.6%',
    dust_pct: 0.1, broad_pct: 2.6, camera_side: false, filters: ['G'], rotations: [3],
    spots: [], before_response: 'g-0910', after_response: 'g-0913',
  }],
  families: [{
    key: 'R:3', filter: 'R', rotation: 3,
    groups: [
      { key: 's-r0920', first_night: '2026-09-20', last_night: '2026-09-20', frames: 10, sessions: [response('r-0920', '2026-09-20', ['s-r0920'])], pairs: [] },
      {
        key: 's-r0907', first_night: '2026-09-07', last_night: '2026-09-16', frames: 20,
        sessions: [
          response('r-0916', '2026-09-16', ['s-r0916']),
          response('r-0907', '2026-09-07', ['s-r0907']),
        ],
        pairs: [{
          earlier_response: 'r-0907', later_response: 'r-0916', earlier_night: '2026-09-07', later_night: '2026-09-16',
          dust_pct: 0.46, broad_pct: 1.05,
        }],
        widest: { dust_pct: 0.46, broad_pct: 1.05, compared: 1 },
      },
    ],
    splits: [{
      before_response: 'r-0916', after_response: 'r-0920', before_night: '2026-09-16', after_night: '2026-09-20',
      start: at('09-16', 2), end: at('09-20'), dust_pct: 1.6, broad_pct: 0.2,
      reason: 'new dust: 1 shadow near (1418, 3540), 1.6% of the background (limit 1%)', event_key: 'e1', spots,
    }],
  }, {
    key: 'G:3', filter: 'G', rotation: 3.2,
    groups: [
      { key: 's-g0913', first_night: '2026-09-13', last_night: '2026-09-13', frames: 10, sessions: [response('g-0913', '2026-09-13', ['s-g0913'])], pairs: [] },
      { key: 's-g0910', first_night: '2026-09-10', last_night: '2026-09-10', frames: 10, sessions: [response('g-0910', '2026-09-10', ['s-g0910'])], pairs: [] },
    ],
    splits: [{
      before_response: 'g-0910', after_response: 'g-0913', before_night: '2026-09-10', after_night: '2026-09-13',
      start: at('09-10', 2), end: at('09-13'), dust_pct: 0.1, broad_pct: 2.6,
      reason: 'the light pattern changed by 2.6% (limit 2%)', event_key: 'e2', spots: [],
    }],
  }],
  unmeasured: [{ night: '2026-09-14', filter: 'r', rotation: 3.4, frames: 2, reason: 'needs 3 flats', sessions: ['s-r0914'] }],
};

const USE: FlatGroupUseView = {
  groups: [{
    key: 's-r0907',
    masters: [{ id: 'm1', label: 'flat-R-0907.fits', created_at: 1, sources: 20, in_group: 20 }],
    lights: { total: 14, own: 10, other_angle: 1, corrected: 0, undetermined: 3, no_same_side: 0 },
    projects: [
      { project_id: 7, name: 'M 44', lights: { total: 12, own: 8, other_angle: 1, corrected: 0, undetermined: 3, no_same_side: 0 } },
      { project_id: 8, name: 'Heart', lights: { total: 2, own: 2, other_angle: 0, corrected: 0, undetermined: 0, no_same_side: 0 } },
    ],
  }],
  computed: true, lights: 20, pending: 4, unread: 2, refreshing: false,
};

function listed(id: string, night: string, filter: string, overrides: Partial<CalibrationSession> = {}): CalibrationSession {
  return {
    id, kind: 'flat', night: `2026-${night}`, rig_uuid: 'rig', rig_name: 'UltraCat', is_master: false,
    filter, exposure_min_s: 2, exposure_max_s: 2, temp_min_c: -10, temp_max_c: -10, gain: 100, offset: 20,
    readout: null, binning: '1x1', width: 6240, height: 4160, rotation: 3, first_at: at(night), last_at: at(night, 2),
    frames: 10, ok: 10, suspect: 0, poor: 0, ungraded: 0, kept_out: 0, ...overrides,
  };
}

const SESSIONS = [
  listed('s-r0920', '09-20', 'R'),
  listed('s-r0916', '09-16', 'R'),
  listed('s-r0914', '09-14', 'R', { frames: 2, ok: 2 }),
  listed('s-g0913', '09-13', 'G'),
  listed('s-g0910', '09-10', 'G'),
  listed('s-r0907', '09-07', 'R'),
  listed('s-d0907', '09-07', '', { kind: 'dark', filter: null, rotation: null }),
];

interface Seen {
  sessions: URLSearchParams[];
  groups: URLSearchParams[];
  use: number;
}

function serve(): Seen {
  const seen: Seen = { sessions: [], groups: [], use: 0 };
  server.use(
    http.get('/api/info', () => ok({ version: 'test', cache_directory: '/cache', allow_database_management: true })),
    http.get('/api/databases', () => ok([
      { id: 'demo', name: 'Demo', database_path: '/demo.sqlite', image_directories: [], remote_image_upload: { enabled: false } },
    ])),
    http.get('/api/db/demo/projects', () => ok([
      { id: 7, profile_id: 'p1', profile_name: 'p1', name: 'M 44', display_name: 'M 44', description: null, has_files: true, state: 1, latest_image_date: null },
    ])),
    http.get('/api/db/demo/calibrations', () => ok({
      schema_version: 9, frame_count: 70, master_count: 1,
      rigs: [{ rig_uuid: 'rig', name: 'UltraCat', frame_count: 70, bias: 0, dark: 10, dark_flat: 0, flat: 60 }],
    })),
    http.get('/api/db/demo/calibrations/project-scope', () => ok({
      project_id: 7, lights: 12, unread: 0, sessions: ['s-r0920'], frames: 10, masters: [],
      rigs: [{ rig_uuid: 'rig', name: 'UltraCat' }], pending: 0, revision: 'r7', refreshing: false,
    })),
    http.get('/api/db/demo/calibrations/sessions', ({ request }) => {
      const params = new URL(request.url).searchParams;
      seen.sessions.push(params);
      const kind = params.get('kind');
      const filter = params.get('filter');
      let sessions = SESSIONS.filter((entry) => (!kind || entry.kind === kind) && (!filter || entry.filter === filter));
      if (params.get('project')) sessions = sessions.filter((entry) => entry.id === 's-r0920');
      if (params.get('session')) sessions = SESSIONS.filter((entry) => entry.id === params.get('session'));
      // Two to a page, so a session can sit on a later one.
      const perPage = 2;
      const locate = params.get('locate');
      const index = locate ? sessions.findIndex((entry) => entry.id === locate) : -1;
      const page = locate ? Math.max(0, Math.floor(index / perPage)) : Number(params.get('page') ?? 0);
      return ok({
        sessions: sessions.slice(page * perPage, page * perPage + perPage), total: sessions.length, page, per_page: perPage,
        frames: sessions.reduce((sum, entry) => sum + entry.frames, 0),
        facets: { filters: ['G', 'R'], exposures: [2], rotations: [3] },
        night_boundary_seconds: 43200,
        ...(locate ? { located: index >= 0 } : {}),
      });
    }),
    http.get('/api/db/demo/calibrations/frames', () => ok({ frames: [], total: 0, page: 0, per_page: 500 })),
    http.get('/api/db/demo/calibrations/flat-groups', ({ request }) => {
      seen.groups.push(new URL(request.url).searchParams);
      return ok({ available: true, rigs: [RIG] });
    }),
    http.get('/api/db/demo/calibrations/flat-groups/place', ({ request }) => {
      const session = new URL(request.url).searchParams.get('session');
      const listed = SESSIONS.find((entry) => entry.id === session);
      return ok(listed
        ? { rig_uuid: listed.rig_uuid, family_key: listed.filter, night: listed.night, rotation: listed.rotation }
        : { rig_uuid: null, family_key: null, night: null, rotation: null });
    }),
    http.get('/api/db/demo/calibrations/flat-groups/use', () => {
      seen.use += 1;
      return ok(USE);
    }),
    http.get('/api/db/demo/calibrations/quality', () => ok({
      progress: { running: false, stage: null, to_measure: 0, done: 0, workers: 0, started_at: null, finished_at: null },
      counts: {},
    })),
    http.post('/api/db/demo/calibrations/generation-status', () => ok({ statuses: [] })),
  );
  return seen;
}

function Location() {
  const location = useLocation();
  return <output aria-label="location">{location.search}</output>;
}

function renderPage(entry: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[entry]}>
        <Routes>
          <Route path="/calibration" element={<>{children}<Location /></>} />
          <Route path="/grid" element={<Location />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return render(<CalibrationPage />, { wrapper });
}

const params = () => new URLSearchParams(screen.getByRole('status', { name: 'location' }).textContent ?? '');
const groupList = () => screen.findByRole('navigation', { name: 'Flat groups' });
const entries = () => within(screen.getByRole('navigation', { name: 'Flat groups' })).getAllByTestId('flat-group-entry');

describe('browsing flat groups', () => {
  beforeAll(() => configure({ asyncUtilTimeout: 10_000 }));
  afterAll(() => configure({ asyncUtilTimeout: 1_000 }));

  it('reads groups by family, newest first, with their marks and the split between them', () => {
    const browse = browseGroups([RIG]);
    expect(browse.families.map((family) => family.label)).toEqual(['R · 3°', 'G · 3.2°']);
    const [r2, r1, g2, g1] = browse.entries;
    expect(groupName(r1)).toBe('Group 1 · R · 3° · 09-07 → 09-16 · 2 nights · 20 flats');
    expect([r2.current, r1.current, g2.current, g1.current]).toEqual([true, false, true, false]);
    // R's older group holds a night too small to measure.
    expect(r1.unmeasured.map((session) => session.night)).toEqual(['2026-09-14']);
    expect(r2.unmeasured).toEqual([]);
    // The dust change follows R's group 1 and, on the camera side, G's newest group.
    expect(r1.newer?.event?.key).toBe('e1');
    expect(g2.newer?.event?.key).toBe('e1');
    expect(r2.newer).toBeUndefined();
    // A member's session finds its group after a regroup renames it.
    expect(browse.find('s-r0916')?.key).toBe('s-r0907');
    expect(browse.find('r-0920')?.key).toBe('s-r0920');
    expect(browseGroups([RIG], { filter: 'g' }).entries.map((entry) => entry.key)).toEqual(['s-g0913', 's-g0910']);
    expect(browseGroups([RIG], { rotation: 54 }).entries).toEqual([]);
  });

  it('tells apart two families of one filter and angle by the night each begins', () => {
    const [r] = RIG.families;
    const other = { ...r, key: 'R:3:gain-0', groups: [{ ...r.groups[0], key: 's-other', first_night: '2026-04-30' }], splits: [] };
    const browse = browseGroups([{ ...RIG, families: [r, other] }]);
    expect(browse.families.map((family) => family.label)).toEqual(['R · 3° · from 2026-09-07', 'R · 3° · from 2026-04-30']);
  });

  it('offers Groups beside Sessions where the list can hold flats, and keeps the choice in the address', async () => {
    serve();
    renderPage('/calibration?db=demo');
    const toggle = await screen.findByRole('group', { name: 'List' });
    expect(within(toggle).getByRole('button', { name: 'Sessions' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(within(toggle).getByRole('button', { name: 'Groups' }));
    await groupList();
    expect(params().get('calBrowse')).toBe('groups');
    // The first group is chosen and named in the address by its key.
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0920'));
    expect(within(toggle).getByRole('button', { name: 'Groups' })).toHaveAttribute('aria-pressed', 'true');

    const list = screen.getByRole('navigation', { name: 'Flat groups' });
    expect(within(list).getAllByTestId('flat-group-family').map((row) => row.textContent)).toEqual(['R · 3°', 'G · 3.2°']);
    expect(entries().map((entry) => entry.getAttribute('aria-label'))).toEqual([
      'Group 2 · R · 3° · 09-20 · 1 night · 10 flats',
      'Group 1 · R · 3° · 09-07 → 09-16 · 2 nights · 20 flats',
      'Group 2 · G · 3.2° · 09-13 · 1 night · 10 flats',
      'Group 1 · G · 3.2° · 09-10 · 1 night · 10 flats',
    ]);
    // The split between R's groups, inline with its reason; and the change
    // after G's newest group.
    expect(within(list).getAllByTestId('flat-group-edge').map((row) => row.textContent?.trim())).toEqual([
      '↕ Change · new dust 1.6%',
      '↕ Change · new dust 1.6%',
      '↕ Change · pattern 2.6%',
    ]);
    const marks = (index: number) => [...entries()[index].querySelectorAll('[data-mark]')].map((mark) => mark.textContent);
    expect(marks(0)).toEqual(['Current']);
    expect(marks(1)).toEqual(['Partly measured', 'Change after']);
    expect(marks(2)).toEqual(['Current', 'Change after']);

    // Type Dark lists no flats: no Groups, and the sessions show.
    fireEvent.change(screen.getByRole('combobox', { name: 'Type' }), { target: { value: 'dark' } });
    await waitFor(() => expect(screen.queryByRole('group', { name: 'List' })).toBeNull());
    expect(await screen.findByRole('navigation', { name: 'Sessions' })).toBeInTheDocument();
  });

  it('restores the group from the address and moves through groups with the arrow keys', async () => {
    serve();
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0907');
    await screen.findByTestId('flat-group-detail');
    expect(within(screen.getByTestId('flat-group-header')).getByRole('heading', { level: 2 })).toHaveTextContent('Group 1 of 2 · R · 3°');
    expect(entries()[1]).toHaveAttribute('aria-current', 'true');

    fireEvent.keyDown(document, { key: 'ArrowDown', code: 'ArrowDown' });
    await waitFor(() => expect(params().get('calGroup')).toBe('s-g0913'));
    fireEvent.keyDown(document, { key: 'ArrowUp', code: 'ArrowUp' });
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0907'));
    fireEvent.keyDown(document, { key: 'ArrowUp', code: 'ArrowUp' });
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0920'));
    // At the top, up stays.
    fireEvent.keyDown(document, { key: 'ArrowUp', code: 'ArrowUp' });
    expect(params().get('calGroup')).toBe('s-r0920');
  });

  it('opens a group by the session a link names, after a regroup', async () => {
    serve();
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0916');
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0907'));
    expect(within(await screen.findByTestId('flat-group-header')).getByRole('heading', { level: 2 }))
      .toHaveTextContent('Group 1 of 2');
  });

  it('shows why a group holds together, its edges, and what it feeds', async () => {
    const seen = serve();
    const opened: OpenSettingsDetail[] = [];
    const listen = (event: Event) => opened.push((event as CustomEvent<OpenSettingsDetail>).detail);
    window.addEventListener(OPEN_SETTINGS_EVENT, listen);
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0907');
    const detail = await screen.findByTestId('flat-group-detail');

    // Members: its nights oldest first, and the night too small to measure.
    expect(within(detail).getAllByTestId('flat-group-member').map((button) => button.querySelector('strong')?.textContent))
      .toEqual(['2026-09-07', '2026-09-16']);
    expect(within(detail).getByTestId('flat-group-unmeasured')).toHaveTextContent('not measured: needs 3 flats');

    // Why: the widest pair against the rig's limits, and a way to change them.
    expect(within(detail).getByTestId('flat-group-widest')).toHaveTextContent(
      'At most 0.46% dust (limit 1%) and 1.1% light pattern (limit 2%) between any two of its nights, over 1 pair measured.');
    expect(within(detail).getByTestId('flat-group-pairs')).toHaveTextContent('2026-09-16 ÷ 2026-09-07');
    fireEvent.click(within(detail).getByTestId('flat-group-edit-limits'));
    expect(opened.at(-1)).toEqual({ intent: 'calibration', field: 'flat_split_dust_pct' });
    window.removeEventListener(OPEN_SETTINGS_EVENT, listen);

    // Edges: the change after it compares across, in the address.
    const newer = within(detail).getByTestId('flat-group-newer-edge');
    expect(newer).toHaveTextContent('After it · Change · new dust 1.6%');
    expect(within(detail).getByText('The oldest group of R · 3°.')).toBeInTheDocument();
    fireEvent.click(within(newer).getByTestId('flat-group-compare'));
    await waitFor(() => expect(params().get('calEarlier')).toBe('r-0916'));
    expect(params().get('calLater')).toBe('r-0920');
    expect(await screen.findByTestId('flat-ratio-viewer')).toBeInTheDocument();
    expect(within(newer).getByTestId('flat-group-compare')).toHaveAttribute('aria-pressed', 'true');

    // What it feeds: its master, and the lights by project, linking to Images.
    const feeds = within(detail).getByTestId('flat-group-feeds');
    expect(await within(feeds).findByRole('link', { name: 'Master built 1970-01-01 00:00 UTC' })).toHaveAttribute(
      'href', expect.stringContaining('calMaster=m1'));
    expect(within(feeds).getByTestId('flat-group-lights')).toHaveTextContent(
      '14 lights take its flats: 10 on their own side, 3 undetermined, 1 from another angle.');
    const links = within(feeds).getAllByTestId('flat-group-lights-link');
    expect(links.map((link) => link.textContent)).toEqual(['M 44: 12 lights', 'Heart: 2 lights']);
    const href = new URL(links[0].getAttribute('href')!, 'http://x');
    expect(href.pathname).toBe('/grid');
    expect(href.searchParams.get('project')).toBe('7');
    // Exactly the counted lights: the grid's flat-group filter, with the
    // database in it; no filter or date span that could catch others.
    expect(href.searchParams.get('db')).toBe('demo');
    expect(href.searchParams.get('flatGroup')).toBe('demo:s-r0907');
    expect(href.searchParams.get('filter')).toBeNull();
    expect(href.searchParams.get('dateStart')).toBeNull();
    const flagged = new URL(within(feeds).getByRole('link', { name: 'the 3 flagged' }).getAttribute('href')!, 'http://x');
    expect(flagged.searchParams.get('flatGroup')).toBe('demo:s-r0907:flagged');
    expect(flagged.searchParams.get('calib')).toBeNull();
    expect(within(feeds).getByTestId('flat-group-unread')).toHaveTextContent('2 lights in the catalog could not be read');
    expect(feeds).toHaveTextContent('4 lights not read by the calibration check yet');
    expect(seen.use).toBe(1);
  });

  it('drops into a member session on the page of the session list that holds it', async () => {
    const seen = serve();
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0907&calFilter=G');
    // Filter G hides R's groups, so G's newest is chosen.
    await screen.findByTestId('flat-group-detail');
    expect(params().get('calGroup')).toBe('s-g0913');
    fireEvent.click(within(screen.getByTestId('flat-group-detail')).getAllByTestId('flat-group-open-session')[0]);
    await waitFor(() => expect(params().get('calSession')).toBe('s-g0913'));
    expect(params().get('calBrowse')).toBeNull();
    expect(params().get('calGroup')).toBe('s-g0913');
    // G's sessions are 09-13 then 09-10, two to a page: the first page.
    expect(params().get('calPage')).toBeNull();
    const located = seen.sessions.find((request) => request.get('locate') === 's-g0913');
    expect(located?.get('filter')).toBe('G');
    const list = await screen.findByRole('navigation', { name: 'Sessions' });
    await waitFor(() => expect(within(list).getByRole('button', { current: true }))
      .toHaveAttribute('data-session-id', 's-g0913'));
  });

  it('finds a session past the first page, and drops a filter that hides it', async () => {
    serve();
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0907');
    const detail = await screen.findByTestId('flat-group-detail');
    // 09-07 R is the sixth session of all: page 2 at two to a page.
    fireEvent.click(within(detail).getAllByTestId('flat-group-open-session')[0]);
    await waitFor(() => expect(params().get('calSession')).toBe('s-r0907'));
    expect(params().get('calPage')).toBe('2');
    const list = await screen.findByRole('navigation', { name: 'Sessions' });
    await waitFor(() => expect(within(list).getByRole('button', { current: true }))
      .toHaveAttribute('data-session-id', 's-r0907'));
  });

  it('in a project scope lists the groups holding its sessions, with their neighbours dimmed', async () => {
    serve();
    renderPage('/calibration?db=demo&project=7&calProject=7&calBrowse=groups');
    await groupList();
    // Only R's newest group holds M 44's session; its older neighbour stays
    // for the edge between them. G holds none.
    await waitFor(() => expect(entries()).toHaveLength(2));
    expect(entries()[0].closest('li')).not.toHaveAttribute('data-context');
    expect(entries()[1].closest('li')).toHaveAttribute('data-context', 'neighbour');
    expect(entries()[1]).toHaveTextContent('Neighbour');
    expect(screen.getByText('1 group · 1 filter and angle')).toBeInTheDocument();
    // The keys stay on the groups in scope.
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0920'));
    fireEvent.keyDown(document, { key: 'ArrowDown', code: 'ArrowDown' });
    expect(params().get('calGroup')).toBe('s-r0920');

    // ✕ on the chip widens to the whole rig, still in Groups.
    fireEvent.click(screen.getAllByRole('button', { name: "Show all of Demo's calibration" })[0]);
    await waitFor(() => expect(params().get('calProject')).toBeNull());
    await waitFor(() => expect(entries()).toHaveLength(4));
    expect(params().get('calBrowse')).toBe('groups');
  });
  // Review repro: while the server works out a newer answer, the detail of
  // a group the stale answer does not name says it feeds nothing, as fact.
  it('review: says the counts are being refreshed rather than that nothing uses the group', async () => {
    serve();
    server.use(http.get('/api/db/demo/calibrations/flat-groups/use', () => ok({ ...USE, refreshing: true })));
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0920');
    const feeds = await screen.findByTestId('flat-group-feeds');
    await waitFor(() => expect(feeds).not.toHaveTextContent('Working out'));
    expect(feeds).not.toHaveTextContent('No light takes its flats.');
    expect(feeds).toHaveTextContent(/refresh|updating|newer/i);
  });

  it('says it is working the counts out, not that nothing uses the group, before the first answer', async () => {
    serve();
    server.use(http.get('/api/db/demo/calibrations/flat-groups/use', () => ok({
      computed: false, groups: [], lights: 0, pending: 0, unread: 0, refreshing: true,
    })));
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0907');
    const feeds = await screen.findByTestId('flat-group-feeds');
    expect(feeds).toHaveTextContent('Working out which masters and lights use it');
    expect(feeds).not.toHaveTextContent('No flat master');
    expect(feeds).not.toHaveTextContent('No light takes its flats');
  });

  it('holds a Groups link until the groups are in, without the session list or a session', async () => {
    serve();
    let release: () => void = () => {};
    const held = new Promise<void>((resolve) => { release = resolve; });
    server.use(http.get('/api/db/demo/calibrations/flat-groups', async () => {
      await held;
      return ok({ available: true, rigs: [RIG] });
    }));
    renderPage('/calibration?db=demo&calBrowse=groups');
    expect(await screen.findByText('Loading groups…')).toBeInTheDocument();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole('navigation', { name: 'Sessions' })).toBeNull();
    expect(params().get('calSession')).toBeNull();
    release();
    await groupList();
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0920'));
    expect(params().get('calSession')).toBeNull();
  });

  it('finds the group of a session no group holds, as when its night was kept out', async () => {
    serve();
    // R 09-14 has too few flats to measure: it lies inside R's group 1.
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0914');
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0907'));
    expect(within(await screen.findByTestId('flat-group-header')).getByRole('heading', { level: 2 }))
      .toHaveTextContent('Group 1 of 2 · R · 3°');
  });

  it('in a project scope dims a group the scope holds only for a correction map', async () => {
    serve();
    server.use(
      http.get('/api/db/demo/calibrations/project-scope', () => ok({
        project_id: 7, lights: 12, unread: 0, sessions: ['s-r0907', 's-g0913'], frames: 20, masters: [],
        rigs: [{ rig_uuid: 'rig', name: 'UltraCat' }], pending: 0, revision: 'r7', refreshing: false,
      })),
      http.get('/api/db/demo/calibrations/flat-groups/use', () => ok({ ...USE, pending: 0, unread: 0 })),
    );
    renderPage('/calibration?db=demo&project=7&calProject=7&calBrowse=groups');
    await groupList();
    await waitFor(() => expect(entries().map((entry) => entry.closest('li')?.getAttribute('data-context') ?? 'own'))
      .toEqual(['neighbour', 'own', 'map', 'neighbour']));
    expect(entries()[2]).toHaveTextContent('Correction map');
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0907'));
  });

  it('calls no group a correction map while some lights are unread', async () => {
    serve();
    server.use(
      http.get('/api/db/demo/calibrations/project-scope', () => ok({
        project_id: 7, lights: 12, unread: 0, sessions: ['s-r0907', 's-g0913'], frames: 20, masters: [],
        rigs: [{ rig_uuid: 'rig', name: 'UltraCat' }], pending: 0, revision: 'r7', refreshing: false,
      })),
      http.get('/api/db/demo/calibrations/flat-groups/use', () => ok({ ...USE, pending: 0, unread: 3 })),
    );
    renderPage('/calibration?db=demo&project=7&calProject=7&calBrowse=groups');
    await groupList();
    await waitFor(() => expect(entries().map((entry) => entry.closest('li')?.getAttribute('data-context') ?? 'own'))
      .toEqual(['neighbour', 'own', 'own', 'neighbour']));
    expect(screen.queryByText('Correction map')).toBeNull();
  });

  it('says it could not work out what a group feeds, and tries again later', async () => {
    serve();
    server.use(http.get('/api/db/demo/calibrations/flat-groups/use', () => ok({
      computed: false, groups: [], lights: 0, pending: 0, unread: 0, refreshing: false,
      error: 'the catalog could not be read',
    })));
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0907');
    const feeds = await screen.findByTestId('flat-group-feeds');
    await waitFor(() => expect(within(feeds).getByTestId('flat-group-use-error'))
      .toHaveTextContent('Could not work out what it feeds: the catalog could not be read'));
    expect(feeds).not.toHaveTextContent('Working out');
  });
});

describe('the frames of a flat group', () => {
  beforeAll(() => configure({ asyncUtilTimeout: 10_000 }));
  afterAll(() => configure({ asyncUtilTimeout: 1_000 }));

  /** Two flats on each of R group 1's nights, 09-07 and 09-16. */
  const flat = (uuid: string, session: string, night: string, minute: number) => ({
    frame_uuid: uuid, rig_uuid: 'rig', kind: 'flat', source_path: `/cal/${uuid}.fits`, source_exists: true,
    captured_at: at(night.slice(5), minute), exposure_s: 2, camera_temp: -10, filter: 'R', grade: 'ok',
    grade_code: null, grade_reason: null, quality_override: null, kept_out: null, valid_direction: null,
    night, session,
  });
  const FLATS = [
    flat('a1', 's-r0907', '2026-09-07', 0), flat('a2', 's-r0907', '2026-09-07', 1),
    flat('b1', 's-r0916', '2026-09-16', 0), flat('b2', 's-r0916', '2026-09-16', 1),
  ];

  function serveFrames(asked: URLSearchParams[]) {
    serve();
    server.use(
      http.get('/api/db/demo/calibrations/frames', ({ request }) => {
        const params = new URL(request.url).searchParams;
        asked.push(params);
        const wanted = params.get('sessions')?.split(',') ?? [params.get('session') ?? ''];
        const frames = FLATS.filter((entry) => wanted.includes(entry.session));
        return ok({ frames, total: frames.length, page: 0, per_page: 500 });
      }),
      http.get('/api/db/demo/calibrations/frames/:uuid', ({ params }) => {
        const own = FLATS.find((entry) => entry.frame_uuid === params.uuid)!;
        return ok({
          ...own, measurement: null, measure_error: null, findings: [], metrics: [],
          session_frames: 2, session_measured: 2, compare: true, masters: 0,
        });
      }),
    );
  }

  const strip = () => screen.findByRole('list', { name: 'Frames' });

  it('holds every flat of the group, night by night, each night marked where it begins', async () => {
    const asked: URLSearchParams[] = [];
    serveFrames(asked);
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0907');
    const list = await strip();
    await waitFor(() => expect(within(list).getAllByRole('button')).toHaveLength(4));
    const request = asked.find((params) => params.get('sessions'));
    expect(request?.get('sessions')).toBe('s-r0907,s-r0916');
    expect(request?.get('sort')).toBe('oldest');
    // The order in the strip: a night's mark, then its flats.
    expect([...list.children].map((item) =>
      item.getAttribute('data-run') ?? item.querySelector('[data-frame-uuid]')?.getAttribute('data-frame-uuid')))
      .toEqual(['s-r0907', 'a1', 'a2', 's-r0916', 'b1', 'b2']);
    expect(within(list).getAllByTestId('calibration-strip-run').map((run) => run.textContent))
      .toEqual(['2026-09-072 flats', '2026-09-162 flats']);
    // The first flat is chosen, in the address beside the group, and the
    // viewer and its numbers show it.
    await waitFor(() => expect(params().get('calFrame')).toBe('a1'));
    expect(params().get('calGroup')).toBe('s-r0907');
    expect(await screen.findByText('Frame 1 of 4 · ← → frames · ↑ ↓ groups · C compare')).toBeInTheDocument();
  });

  it('steps from one night to the next with the arrow keys, and a member card brings its night', async () => {
    serveFrames([]);
    renderPage('/calibration?db=demo&calBrowse=groups&calGroup=s-r0907&calFrame=a2');
    const list = await strip();
    await waitFor(() => expect(within(list).getByRole('button', { pressed: true }))
      .toHaveAttribute('data-frame-uuid', 'a2'));
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(params().get('calFrame')).toBe('b1'));
    expect(screen.getByText(/^Frame 3 of 4/)).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'ArrowLeft', code: 'ArrowLeft' });
    await waitFor(() => expect(params().get('calFrame')).toBe('a2'));
    // ↑ ↓ stay with the groups.
    fireEvent.keyDown(document, { key: 'ArrowUp', code: 'ArrowUp' });
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0920'));
    fireEvent.keyDown(document, { key: 'ArrowDown', code: 'ArrowDown' });
    await waitFor(() => expect(params().get('calGroup')).toBe('s-r0907'));

    // The 09-16 card shows that night's first flat, and stays in the group.
    const detail = await screen.findByTestId('flat-group-detail');
    const cards = within(detail).getAllByTestId('flat-group-member');
    fireEvent.click(cards[1]);
    await waitFor(() => expect(params().get('calFrame')).toBe('b1'));
    expect(params().get('calBrowse')).toBe('groups');
    await waitFor(() => expect(within(detail).getAllByTestId('flat-group-member')[1])
      .toHaveAttribute('aria-current', 'true'));
  });
});
