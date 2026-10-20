import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { delay, http, HttpResponse } from 'msw';
import { createMemoryRouter, RouterProvider, useLocation, useSearchParams } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { apiClient } from '../../../api/client';
import { AccessContext, useAccess } from '../../../auth/access';
import { server } from '../../../test/msw-server';
import CalibrationPage from '../CalibrationPage';
import { serverTime } from '../calibrationLabels';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

function session(id: string, overrides: Record<string, unknown> = {}) {
  return {
    id, kind: 'flat', night: '2026-09-10', rig_uuid: 'rig', rig_name: 'Askar · ASI2600', is_master: false,
    filter: 'Ha', exposure_min_s: 2, exposure_max_s: 2.4, temp_min_c: -10, temp_max_c: -10,
    gain: 100, offset: 20, readout: null, binning: '1x1', width: 6248, height: 4176, rotation: 92,
    first_at: 1757563200, last_at: 1757563800, frames: 3, ok: 2, suspect: 0, poor: 1, ungraded: 0, kept_out: 1,
    ...overrides,
  };
}

function frame(uuid: string, sessionId: string, overrides: Record<string, unknown> = {}) {
  return {
    frame_uuid: uuid, rig_uuid: 'rig', kind: 'flat', source_path: `/cal/${uuid}.fits`, source_exists: true,
    captured_at: 1757563200, exposure_s: 2, camera_temp: -10, filter: 'Ha', grade: 'ok', grade_code: null,
    grade_reason: null, quality_override: null, kept_out: null, valid_direction: null,
    night: '2026-09-10', session: sessionId,
    ...overrides,
  };
}

function detail(uuid: string, sessionId: string, overrides: Record<string, unknown> = {}) {
  return {
    ...frame(uuid, sessionId),
    measurement: { type: 'flat', width: 6248, height: 4176, feature_count: 1,
      features: [{ x: 100, y: 120, left: 90, top: 110, right: 110, bottom: 130, pixels: 300, depth: 0.12 }] },
    measure_error: null,
    findings: [],
    metrics: [
      { key: 'level_fraction', value: 0.42, session: 0.41 },
      { key: 'features', value: 1, session: 1 },
    ],
    session_frames: 3,
    session_measured: 3,
    compare: true,
    masters: 2,
    ...overrides,
  };
}

const SESSIONS = [session('s1'), session('s2', { kind: 'dark', filter: null, exposure_min_s: 300, exposure_max_s: 300, poor: 0, kept_out: 0 })];
const FRAMES: Record<string, ReturnType<typeof frame>[]> = {
  s1: [
    frame('f1', 's1'),
    frame('f2', 's1', { grade: 'poor', grade_code: 'changing_features', kept_out: '30 spots that change', grade_reason: '30 spots that change across its session' }),
    frame('f3', 's1'),
  ],
  s2: [frame('d1', 's2', { kind: 'dark' }), frame('d2', 's2', { kind: 'dark' })],
};

interface Seen {
  sessions: URLSearchParams[];
  frames: URLSearchParams[];
  choices: unknown[];
  validity: unknown[];
  forgotten: string[];
}

function serve({ manage = true }: { manage?: boolean } = {}): Seen {
  const seen: Seen = { sessions: [], frames: [], choices: [], validity: [], forgotten: [] };
  server.use(
    http.get('/api/info', () => ok({ version: 'test', cache_directory: '/cache', allow_database_management: manage })),
    http.get('/api/databases', () => ok([
      { id: 'demo', name: 'Demo', database_path: '/demo.sqlite', image_directories: [], remote_image_upload: { enabled: false } },
    ])),
    http.get('/api/db/demo/calibrations', () => ok({
      schema_version: 9, frame_count: 5, master_count: 1,
      rigs: [{
        rig_uuid: 'rig', name: 'Askar · ASI2600', frame_count: 5, bias: 0, dark: 2, dark_flat: 0, flat: 3,
        // The rig computer keeps PDT.
        clock: [{ from: 0, offset_seconds: -7 * 3600 }],
      }],
    })),
    http.get('/api/db/demo/calibrations/sessions', ({ request }) => {
      const params = new URL(request.url).searchParams;
      seen.sessions.push(params);
      const kind = params.get('kind');
      const sessions = SESSIONS.filter((entry) => !kind || entry.kind === kind);
      return ok({
        sessions, total: sessions.length, page: 0, per_page: 50,
        frames: sessions.reduce((sum, entry) => sum + entry.frames, 0),
        facets: { filters: ['Ha', 'OIII'], exposures: [2, 300], first_night: '2026-09-01', last_night: '2026-09-12' },
        night_boundary_seconds: 43200,
      });
    }),
    http.get('/api/db/demo/calibrations/frames', ({ request }) => {
      const params = new URL(request.url).searchParams;
      seen.frames.push(params);
      const frames = FRAMES[params.get('session') ?? ''] ?? [];
      return ok({ frames, total: frames.length, page: 0, per_page: 500 });
    }),
    http.get('/api/db/demo/calibrations/frames/:uuid', ({ params }) => {
      const uuid = String(params.uuid);
      const sessionId = Object.keys(FRAMES).find((key) => FRAMES[key].some((entry) => entry.frame_uuid === uuid)) ?? 's1';
      const own = FRAMES[sessionId].find((entry) => entry.frame_uuid === uuid);
      return ok(detail(uuid, sessionId, {
        ...own,
        ...(own?.kind === 'dark' ? { compare: false, measurement: null, metrics: [{ key: 'noise', value: 4.2, session: 4.1 }] } : {}),
        ...(uuid === 'f2' ? { findings: [{ code: 'changing_features', grade: 'poor', value: 30, limit: 20, text: '30 spots that change across its session' }] } : {}),
      }));
    }),
    http.put('/api/db/demo/calibrations/frames/quality', async ({ request }) => {
      seen.choices.push(await request.json());
      return ok({ updated: 1 });
    }),
    http.put('/api/db/demo/calibrations/frames/validity', async ({ request }) => {
      seen.validity.push(await request.json());
      return ok({ updated: 1 });
    }),
    http.delete('/api/db/demo/calibrations/frames/:uuid', ({ params }) => {
      seen.forgotten.push(String(params.uuid));
      return ok({ frames_removed: 1, masters_removed: 0 });
    }),
    http.get('/api/db/demo/calibrations/masters', () => ok({
      notes: [],
      masters: [{
        id: 'm1', kind: 'flat', label: `flat-${'a'.repeat(64)}.fits`, available: true, unavailable_reason: null,
        usages: [], preview_url: '/api/db/demo/calibrations/masters/m1/preview',
        original_preview_url: '/api/db/demo/calibrations/masters/m1/preview?size=original',
        fits_url: '/api/db/demo/calibrations/masters/m1/fits', source_count: 20, rejection_method: 'median_mad',
        masked_samples: null, rejected_samples: 12, minimum_clean_samples: null, maximum_clean_samples: null,
        sources: { frames: 20, first_night: '2026-09-10', last_night: '2026-09-10', nights: 1 },
        rig_uuid: 'rig', rig_name: 'Askar · ASI2600', created_at: 1757600000, exposure_s: null, filter: 'Ha',
        camera_temp: -10, thin: false, external: false, defect_pixels_replaced: 3,
        flat_set: { measured_frames: 20, frames: 20, master: { grade: 'poor', findings: [] }, steady_features: 4, changing_features: 31, carried_features: 25 },
        flat_note: null, set_aside: false, used_by: [{ project_id: 1, name: 'Heart' }],
      }, {
        id: 'm2', kind: 'dark', label: `dark-${'b'.repeat(64)}.fits`, available: false,
        unavailable_reason: 'gone', usages: [], preview_url: null, original_preview_url: null, fits_url: null,
        source_count: 2, rejection_method: null, masked_samples: null, rejected_samples: null,
        minimum_clean_samples: null, maximum_clean_samples: null, rig_uuid: 'rig', rig_name: null,
        created_at: 1757500000, exposure_s: 300, filter: null, camera_temp: -10, thin: true, external: false,
        defect_pixels_replaced: null, flat_set: null, flat_note: null, set_aside: false, used_by: [],
      }],
    })),
    http.post('/api/db/demo/calibrations/generation-status', () => ok({ statuses: [] })),
  );
  return seen;
}

function Location() {
  const location = useLocation();
  return <output aria-label="location">{location.search}</output>;
}

/** What the header's project picker does: another project, maybe in
 *  another database. */
function Scope() {
  const [, setSearchParams] = useSearchParams();
  const open = (db: string, project: string) =>
    setSearchParams((previous) => {
      const next = new URLSearchParams(previous);
      next.set('db', db);
      next.set('project', project);
      return next;
    }, { replace: true });
  return (
    <>
      <button type="button" onClick={() => open('demo', '7')}>Open Bubble</button>
      <button type="button" onClick={() => open('demo', '8')}>Open Heart</button>
      <button type="button" onClick={() => open('other', '9')}>Open Iris</button>
    </>
  );
}

const TWO_RIGS = {
  schema_version: 9, frame_count: 9, master_count: 1,
  rigs: [
    { rig_uuid: 'rig', name: 'Askar · ASI2600', profile_ids: ['p1'], frame_count: 5, bias: 0, dark: 2, dark_flat: 0, flat: 3 },
    { rig_uuid: 'rig2', name: 'C925 · ASI6200', profile_ids: ['p2', 'p3'], frame_count: 4, bias: 0, dark: 2, dark_flat: 0, flat: 2 },
  ],
};

function project(id: number, profile: string, name: string) {
  return { id, profile_id: profile, profile_name: profile, name, display_name: name, description: null, has_files: true, state: 1, latest_image_date: null };
}

/** Demo with two rigs and two projects, and a second database whose
 *  library holds `otherRigs`. */
function serveTwoDatabases(otherRigs: string[]) {
  const seen = serve();
  const other: URLSearchParams[] = [];
  server.use(
    http.get('/api/databases', () => ok([
      { id: 'demo', name: 'Demo', database_path: '/demo.sqlite', image_directories: [], remote_image_upload: { enabled: false } },
      { id: 'other', name: 'Other', database_path: '/other.sqlite', image_directories: [], remote_image_upload: { enabled: false } },
    ])),
    http.get('/api/db/demo/calibrations', () => ok(TWO_RIGS)),
    http.get('/api/db/demo/projects', () => ok([project(7, 'p3', 'Bubble'), project(8, 'p1', 'Heart')])),
    http.get('/api/db/other/calibrations', () => ok({
      schema_version: 9, frame_count: 4, master_count: 0,
      rigs: otherRigs.map((rig) => ({
        rig_uuid: rig, name: rig, profile_ids: [`${rig}-profile`], frame_count: 2, bias: 0, dark: 2, dark_flat: 0, flat: 0,
      })),
    })),
    http.get('/api/db/other/projects', () => ok([project(9, `${otherRigs.at(-1)}-profile`, 'Iris')])),
    http.get('/api/db/other/calibrations/sessions', ({ request }) => {
      other.push(new URL(request.url).searchParams);
      return ok({
        sessions: [], total: 0, page: 0, per_page: 50, frames: 0,
        facets: { filters: [], exposures: [], first_night: null, last_night: null },
        night_boundary_seconds: 43200,
      });
    }),
  );
  return { ...seen, other };
}

function WithAccess({ canWrite, children }: { canWrite: boolean; children: ReactNode }) {
  const access = useAccess();
  return <AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider>;
}

function renderPage(entry: string, { canWrite = true }: { canWrite?: boolean } = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  // The app's kind of router: a data router.
  const router = createMemoryRouter(
    [{ path: '/calibration', element: <><CalibrationPage /><Location /><Scope /></> }],
    { initialEntries: [entry] },
  );
  return render(
    <QueryClientProvider client={client}>
      <WithAccess canWrite={canWrite}>
        <RouterProvider router={router} />
      </WithAccess>
    </QueryClientProvider>,
  );
}

const params = () => new URLSearchParams(screen.getByRole('status', { name: 'location' }).textContent ?? '');

afterEach(() => vi.restoreAllMocks());

describe('CalibrationPage', () => {
  it('opens on the first session and its first frame, and keeps both in the URL', async () => {
    serve();
    renderPage('/calibration?db=demo');
    // The file's first render pays for loading the page under a busy suite.
    await waitFor(() => expect(params().get('calSession')).toBe('s1'), { timeout: 5000 });
    await waitFor(() => expect(params().get('calFrame')).toBe('f1'));
    const sessions = screen.getByRole('navigation', { name: 'Sessions' });
    expect(within(sessions).getByRole('button', { current: true })).toHaveTextContent('Flats · Ha');
    expect(within(sessions).getByText('1 poor')).toBeInTheDocument();
    const strip = screen.getByRole('list', { name: 'Frames' });
    expect(within(strip).getAllByRole('button')).toHaveLength(3);
    expect(within(strip).getByRole('button', { name: 'Frame 1: f1.fits' })).toHaveAttribute('aria-pressed', 'true');
    // The panel sets the frame's numbers beside its session's.
    const panel = await screen.findByRole('complementary', { name: 'Frame' });
    expect(await within(panel).findByRole('heading', { name: 'f1.fits' })).toBeInTheDocument();
    expect(within(panel).getByRole('row', { name: /Level 42.0% 41.0%/ })).toBeInTheDocument();
    // Its time, and its thumbnail's, read on the rig's clock: 04:00 UTC.
    expect(await within(panel).findByText('Flat · 2025-09-10 21:00 UTC−7')).toBeInTheDocument();
    expect(within(strip).getByRole('button', { name: 'Frame 1: f1.fits' }).getAttribute('title'))
      .toContain('2025-09-10 21:00 UTC−7');
  });

  it('restores a shared view: session, frame, comparison and stretch', async () => {
    serve();
    renderPage('/calibration?db=demo&calSession=s1&calFrame=f2&calView=session&calStretch=tight');
    const panel = await screen.findByRole('complementary', { name: 'Frame' });
    expect(await within(panel).findByRole('heading', { name: 'f2.fits' })).toBeInTheDocument();
    expect(within(panel).getByText('Poor · changing spots')).toBeInTheDocument();
    expect(within(panel).getByText(/30 spots that change across its session/, { selector: 'li' })).toHaveTextContent('limit 20');
    expect(screen.getByRole('button', { name: 'Frame 2: f2.fits' })).toHaveAttribute('aria-pressed', 'true');
    const compare = await screen.findByRole('button', { name: 'Compare with session' });
    await waitFor(() => expect(compare).toHaveAttribute('aria-pressed', 'true'));
    expect(within(screen.getByRole('group', { name: 'Stretch' })).getByRole('button', { name: 'Tight' }))
      .toHaveAttribute('aria-pressed', 'true');
    // The picture asked for is the comparison, tightly stretched.
    const viewer = screen.getByRole('region', { name: 'Frame viewer' });
    const probe = viewer.querySelector('img.calibration-probe') as HTMLImageElement;
    const url = new URL(probe.src);
    expect(url.pathname).toBe('/api/db/demo/calibrations/frames/f2/preview');
    expect(Object.fromEntries(url.searchParams)).toEqual({ view: 'session', size: 'screen', stretch: 'tight' });
    expect(params().get('calFrame')).toBe('f2');
  });

  it("opens on the rig of the project in scope, and keeps All once chosen", async () => {
    const seen = serve();
    server.use(
      http.get('/api/db/demo/calibrations', () => ok({
        schema_version: 9, frame_count: 9, master_count: 1,
        rigs: [
          { rig_uuid: 'rig', name: 'Askar · ASI2600', profile_ids: ['p1'], frame_count: 5, bias: 0, dark: 2, dark_flat: 0, flat: 3 },
          { rig_uuid: 'rig2', name: 'C925 · ASI6200', profile_ids: ['p2', 'p3'], frame_count: 4, bias: 0, dark: 2, dark_flat: 0, flat: 2 },
        ],
      })),
      http.get('/api/db/demo/projects', () => ok([
        { id: 7, profile_id: 'p3', profile_name: 'C925', name: 'Bubble', display_name: 'Bubble', description: null, has_files: true, state: 1, latest_image_date: null },
      ])),
    );
    renderPage('/calibration?db=demo&project=7');
    await waitFor(() => expect(params().get('calRig')).toBe('rig2'));
    await waitFor(() => expect(seen.sessions.at(-1)?.get('rig')).toBe('rig2'));
    expect(screen.queryByRole('combobox', { name: 'Database' })).not.toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'any' } });
    await waitFor(() => expect(params().get('calRig')).toBe('any'));
    await waitFor(() => expect(seen.sessions.at(-1)?.has('rig')).toBe(false));
  });

  it('opens each project on its own rig, and keeps a rig chosen by hand', async () => {
    serveTwoDatabases(['o1', 'o2']);
    renderPage('/calibration?db=demo&project=7');
    await waitFor(() => expect(params().get('calRig')).toBe('rig2'));
    // Another project: the rig the last one opened on gives way to its own.
    fireEvent.click(screen.getByRole('button', { name: 'Open Heart' }));
    await waitFor(() => expect(params().get('project')).toBe('8'));
    await waitFor(() => expect(params().get('calRig')).toBe('rig'));
    // A rig chosen by hand stays when the project changes.
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'any' } });
    await waitFor(() => expect(params().get('calRig')).toBe('any'));
    fireEvent.click(screen.getByRole('button', { name: 'Open Bubble' }));
    await waitFor(() => expect(params().get('project')).toBe('7'));
    await screen.findByLabelText('Rig');
    expect(params().get('calRig')).toBe('any');
  });

  it("drops the last database's rig and filters, and opens the new project's rig", async () => {
    const seen = serveTwoDatabases(['o1', 'o2']);
    renderPage('/calibration?db=demo&project=8&calKind=dark&calGrade=poor');
    await waitFor(() => expect(params().get('calRig')).toBe('rig'));
    fireEvent.click(screen.getByRole('button', { name: 'Open Iris' }));
    await waitFor(() => expect(params().get('db')).toBe('other'));
    await waitFor(() => expect(params().get('calRig')).toBe('o2'));
    expect(params().get('calKind')).toBeNull();
    expect(params().get('calGrade')).toBeNull();
    await waitFor(() => expect(seen.other.at(-1)?.get('rig')).toBe('o2'));
    expect(seen.other.at(-1)?.has('kind')).toBe(false);
  });

  it("drops the last database's rig where the new one has a single rig", async () => {
    const seen = serveTwoDatabases(['o1']);
    const sessions = vi.spyOn(apiClient, 'getCalibrationSessions');
    renderPage('/calibration?db=demo&project=7');
    await waitFor(() => expect(params().get('calRig')).toBe('rig2'));
    fireEvent.click(screen.getByRole('button', { name: 'Open Iris' }));
    await waitFor(() => expect(params().get('db')).toBe('other'));
    await waitFor(() => expect(params().get('calRig')).toBeNull());
    // Requests can reach MSW out of order. Check the latest query at its
    // call site, and that the unfiltered request reached the server.
    await waitFor(() => {
      const calls = sessions.mock.calls.filter(([dbId]) => dbId === 'other');
      expect(calls.length).toBeGreaterThan(0);
      expect(calls.at(-1)?.[1]?.rig).toBeUndefined();
      expect(seen.other.some((request) => !request.has('rig'))).toBe(true);
    });
    // One rig: no Rig choice, and nothing hidden left choosing.
    expect(screen.queryByLabelText('Rig')).toBeNull();
    // Back in the first database, the project opens on its rig again.
    fireEvent.click(screen.getByRole('button', { name: 'Open Bubble' }));
    await waitFor(() => expect(params().get('db')).toBe('demo'));
    await waitFor(() => expect(params().get('calRig')).toBe('rig2'));
  });

  it('writes filters to the URL and asks the server for them', async () => {
    const seen = serve();
    // Requests can reach MSW out of order: on a CI runner the request for
    // Type and Grade alone landed after the one with the temperature too.
    // Hold it back until that one has landed, so every run meets the order
    // that failed there.
    let temperatureLanded = () => {};
    const landed = new Promise<void>((resolve) => { temperatureLanded = resolve; });
    server.use(
      http.get('/api/db/demo/calibrations/sessions', async ({ request }) => {
        const query = new URL(request.url).searchParams;
        if (query.has('temp_min')) temperatureLanded();
        else if (query.get('grade') === 'poor') await landed;
        // Nothing returned: serve()'s handler answers.
      }),
    );
    const asked = vi.spyOn(apiClient, 'getCalibrationSessions');
    renderPage('/calibration?db=demo');
    await waitFor(() => expect(params().get('calSession')).toBe('s1'));
    fireEvent.change(screen.getByLabelText('Type'), { target: { value: 'dark' } });
    await waitFor(() => expect(params().get('calKind')).toBe('dark'));
    await waitFor(() => expect(seen.sessions.at(-1)?.get('kind')).toBe('dark'));
    fireEvent.change(screen.getByLabelText('Grade'), { target: { value: 'poor' } });
    await waitFor(() => expect(params().get('calGrade')).toBe('poor'));
    const from = screen.getByLabelText('From °C');
    fireEvent.change(from, { target: { value: '-12' } });
    fireEvent.blur(from);
    // The page's latest query carries every filter, read at its call site
    // since the server may hear an older one last…
    await waitFor(() =>
      expect(asked.mock.calls.at(-1)?.[1]).toMatchObject({ kind: 'dark', grade: 'poor', temp_min: -12 }));
    // …and the server was asked for all of them at once.
    await waitFor(() => expect(seen.sessions.some((request) =>
      request.get('kind') === 'dark' && request.get('grade') === 'poor' && request.get('temp_min') === '-12',
    )).toBe(true));
    fireEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
    await waitFor(() => expect(params().get('calKind')).toBeNull());
    expect(params().get('calGrade')).toBeNull();
  });

  it('gives way to the first session left when the filters drop the chosen one', async () => {
    serve();
    renderPage('/calibration?db=demo&calSession=s2&calFrame=d1&calKind=flat');
    await waitFor(() => expect(params().get('calSession')).toBe('s1'));
    await waitFor(() => expect(params().get('calFrame')).toBe('f1'));
  });

  it('flips frames with left and right and sessions with up and down', async () => {
    serve();
    renderPage('/calibration?db=demo');
    await waitFor(() => expect(params().get('calFrame')).toBe('f1'));
    await screen.findByRole('heading', { name: 'f1.fits' });
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(params().get('calFrame')).toBe('f2'));
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(params().get('calFrame')).toBe('f3'));
    // The last frame stays the last.
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    fireEvent.keyDown(document, { key: 'ArrowLeft', code: 'ArrowLeft' });
    await waitFor(() => expect(params().get('calFrame')).toBe('f2'));
    fireEvent.keyDown(document, { key: 'c', code: 'KeyC' });
    await waitFor(() => expect(params().get('calView')).toBe('session'));
    fireEvent.keyDown(document, { key: 'ArrowDown', code: 'ArrowDown' });
    await waitFor(() => expect(params().get('calSession')).toBe('s2'));
    await waitFor(() => expect(params().get('calFrame')).toBe('d1'));
    expect(params().get('calView')).toBeNull();
  });

  it('stays in compare mode while stepping, though the next frame is still loading', async () => {
    serve();
    renderPage('/calibration?db=demo&calSession=s1&calFrame=f1&calView=session');
    await screen.findByRole('heading', { name: 'f1.fits' });
    const compare = await screen.findByRole('button', { name: 'Compare with session' });
    await waitFor(() => expect(compare).toHaveAttribute('aria-pressed', 'true'));
    // The next frames' details arrive late, as from a busy server.
    server.use(http.get('/api/db/demo/calibrations/frames/:uuid', async ({ params }) => {
      await delay(150);
      const uuid = String(params.uuid);
      return ok(detail(uuid, 's1', FRAMES.s1.find((entry) => entry.frame_uuid === uuid)));
    }));
    const asked = vi.spyOn(apiClient, 'getCalibrationFramePreviewUrl');
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(params().get('calFrame')).toBe('f2'));
    await screen.findByRole('heading', { name: 'f2.fits' });
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(params().get('calFrame')).toBe('f3'));
    await screen.findByRole('heading', { name: 'f3.fits' });
    // The viewer's pictures, not the strip's thumbnails.
    const viewer = asked.mock.calls.filter(([, , options]) => options.size !== 'thumb');
    expect(viewer.map(([, uuid]) => uuid)).toContain('f2');
    expect(viewer.map(([, , options]) => options.view)).not.toContain('frame');
    expect(compare).toHaveAttribute('aria-pressed', 'true');
    expect(params().get('calView')).toBe('session');
  });

  it('draws large dust at its shape, from a toggle nested under Spots, and keeps both in the URL', async () => {
    serve();
    const features = [
      { x: 3080, y: 2970, left: 2785, top: 2675, right: 3375, bottom: 3265, pixels: 270000, depth: 0.0033,
        shape: 'disk', radius: 295, distance_mm: 10.9 },
      { x: 500, y: 2050, left: 380, top: 1930, right: 620, bottom: 2170, pixels: 45000, depth: 0.016,
        shape: 'donut', radius: 120, hole: 0.12 },
      { x: 100, y: 120, left: 90, top: 110, right: 110, bottom: 130, pixels: 300, depth: 0.12, shape: 'compact', radius: 10 },
    ];
    server.use(http.get('/api/db/demo/calibrations/frames/:uuid', ({ params }) => ok(detail(String(params.uuid), 's1', {
      measurement: {
        type: 'flat', width: 6248, height: 4176, feature_count: 3, features,
        vignetting: [{ edge: 'bottom_left', x: 170, y: 3940, area: 180000, depth: 0.024 }],
      },
      metrics: [{ key: 'features', value: 1, session: 1 }, { key: 'large_dust', value: 2, session: 2 }],
    }))));
    renderPage('/calibration?db=demo&calSession=s1&calFrame=f1');
    const panel = await screen.findByRole('complementary', { name: 'Frame' });
    // Spots and large dust read as one row, the frame's and the session's.
    expect(await within(panel).findByRole('row', { name: 'Spots 1 spot · 2 large disks 1 spot · 2 large disks' }))
      .toBeInTheDocument();

    const viewer = screen.getByRole('region', { name: 'Frame viewer' });
    const spots = await within(viewer).findByRole('group', { name: 'Spots' });
    const spotsButton = within(spots).getByRole('button', { name: 'Spots' });
    expect(spotsButton).toHaveAttribute('aria-pressed', 'false');
    // The large-dust toggle sits under Spots, offered only while Spots is on.
    expect(within(spots).queryByRole('button', { name: 'Large dust' })).toBeNull();
    fireEvent.click(spotsButton);
    await waitFor(() => expect(params().get('calSpots')).toBe('1'));
    const dustButton = await within(spots).findByRole('button', { name: 'Large dust' });
    expect(dustButton).toHaveAttribute('aria-pressed', 'true');

    // The picture loads at a size the overlay scales to.
    const probe = viewer.querySelector('img.calibration-probe') as HTMLImageElement;
    fireEvent.load(probe);
    const shown = await within(viewer).findByTestId('calibration-frame-image');
    Object.defineProperty(shown, 'naturalWidth', { value: 1562 });
    Object.defineProperty(shown, 'naturalHeight', { value: 1044 });
    fireEvent.load(shown);
    const overlay = await within(viewer).findByTestId('calibration-dust-overlay');
    expect(overlay).toHaveAttribute('viewBox', '0 0 6248 4176');
    const marks = within(overlay).getAllByRole('button');
    expect(marks.map((mark) => mark.getAttribute('data-shape')).sort()).toEqual(['compact', 'disk', 'donut', 'vignetting']);
    // A disk is its rim; a donut its ring and its hole.
    const disk = marks.find((mark) => mark.getAttribute('data-shape') === 'disk')!;
    expect(disk.querySelector('circle')).toHaveAttribute('r', '295');
    const donut = marks.find((mark) => mark.getAttribute('data-shape') === 'donut')!;
    expect(donut.querySelectorAll('circle')).toHaveLength(2);
    // Its label: in full on a wide picture, the depth alone on a small one.
    expect(disk.querySelector('text')?.textContent).toMatch(/^(disk 0\.33% · ~11 mm, filter\/window|0\.33%)$/);
    expect(disk).toHaveAttribute('aria-label', expect.stringContaining('disk 0.33% · ~11 mm, filter/window'));

    // A tap says the numbers.
    fireEvent.click(disk);
    expect(await within(viewer).findByTestId('calibration-dust-pick'))
      .toHaveTextContent('disk 0.33% · ~11 mm, filter/window · 590 px across · at (3080, 2970)');

    // Large dust off: the compact spot stays.
    fireEvent.click(dustButton);
    await waitFor(() => expect(params().get('calDust')).toBe('0'));
    await waitFor(() => expect(within(overlay).getAllByRole('button')).toHaveLength(1));
    expect(within(spots).getByRole('button', { name: 'Large dust' })).toHaveAttribute('aria-pressed', 'false');
  });

  it('restores spots and large dust from a shared link, and draws the session dust against the session', async () => {
    serve();
    server.use(http.get('/api/db/demo/calibrations/frames/:uuid', ({ params }) => ok(detail(String(params.uuid), 's1', {
      session_dust: {
        bin: 7, frames: 10,
        features: [
          { x: 3080, y: 2970, left: 2785, top: 2675, right: 3375, bottom: 3265, pixels: 270000, depth: 0.0033,
            shape: 'disk', radius: 295, seen_in: { frames: 10, of: 10 } },
          { x: 4040, y: 1900, left: 3745, top: 1605, right: 4335, bottom: 2195, pixels: 270000, depth: 0.0025,
            shape: 'disk', radius: 295, seen_in: { frames: 9, of: 10 } },
        ],
        vignetting: [],
        seen: [0],
        transients: [{ x: 900, y: 800, left: 890, top: 790, right: 910, bottom: 810, pixels: 400, depth: 0.05, shape: 'compact', radius: 11 }],
      },
    }))));
    renderPage('/calibration?db=demo&calSession=s1&calFrame=f1&calView=session&calSpots=1');
    const viewer = screen.getByRole('region', { name: 'Frame viewer' });
    const spots = await within(viewer).findByRole('group', { name: 'Spots' });
    expect(within(spots).getByRole('button', { name: 'Spots' })).toHaveAttribute('aria-pressed', 'true');
    expect(await within(spots).findByRole('button', { name: 'Large dust' })).toHaveAttribute('aria-pressed', 'true');
    const probe = viewer.querySelector('img.calibration-probe') as HTMLImageElement;
    fireEvent.load(probe);
    const shown = await within(viewer).findByTestId('calibration-frame-image');
    Object.defineProperty(shown, 'naturalWidth', { value: 1562 });
    Object.defineProperty(shown, 'naturalHeight', { value: 1044 });
    fireEvent.load(shown);
    const overlay = await within(viewer).findByTestId('calibration-dust-overlay');
    const marks = within(overlay).getAllByRole('button');
    expect(marks).toHaveLength(3);
    // The session's disks, each with the flats it shows in; what this flat
    // alone holds is a transient, not dust.
    expect(marks.find((mark) => mark.getAttribute('aria-label')?.includes('in 9 of 10 flats'))?.getAttribute('aria-label'))
      .toContain('not seen in this flat');
    expect(marks.find((mark) => mark.getAttribute('data-shape') === 'compact')).toHaveClass('is-transient');
  });

  // A mark tapped on one frame must not describe another frame's mark
  // after stepping with the arrow keys.
  it('forgets the picked mark when the frame changes', async () => {
    serve();
    server.use(http.get('/api/db/demo/calibrations/frames/:uuid', ({ params }) => {
      const uuid = String(params.uuid);
      const x = uuid === 'f1' ? 100 : 4000;
      return ok(detail(uuid, 's1', {
        ...FRAMES.s1.find((entry) => entry.frame_uuid === uuid),
        measurement: { type: 'flat', width: 6248, height: 4176, feature_count: 1,
          features: [{ x, y: 120, left: x - 10, top: 110, right: x + 10, bottom: 130, pixels: 300, depth: 0.12, shape: 'compact', radius: 10 }] },
      }));
    }));
    renderPage('/calibration?db=demo&calSession=s1&calFrame=f1&calSpots=1');
    await screen.findByRole('heading', { name: 'f1.fits' });
    const viewer = screen.getByRole('region', { name: 'Frame viewer' });
    fireEvent.load(viewer.querySelector('img.calibration-probe') as HTMLImageElement);
    const shown = await within(viewer).findByTestId('calibration-frame-image');
    Object.defineProperty(shown, 'naturalWidth', { value: 1562 });
    Object.defineProperty(shown, 'naturalHeight', { value: 1044 });
    fireEvent.load(shown);
    const overlay = await within(viewer).findByTestId('calibration-dust-overlay');
    fireEvent.click(within(overlay).getAllByRole('button')[0]);
    expect(await within(viewer).findByTestId('calibration-dust-pick')).toHaveTextContent('at (100, 120)');
    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await screen.findByRole('heading', { name: 'f2.fits' });
    // The overlay waits for f2's own picture.
    expect(within(viewer).queryByTestId('calibration-dust-overlay')).toBeNull();
    fireEvent.load(viewer.querySelector('img.calibration-probe') as HTMLImageElement);
    await waitFor(() => expect(within(viewer).getByTestId('calibration-dust-overlay')
      .querySelector('[data-shape="compact"]')).toHaveAttribute('aria-label', expect.stringContaining('(4000, 120)')));
    // Nothing was tapped on f2.
    expect(within(viewer).queryByTestId('calibration-dust-pick')).toBeNull();
  });

  it('keeps a frame out, marks its validity and forgets it', async () => {
    const seen = serve();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage('/calibration?db=demo&calSession=s1&calFrame=f1');
    const panel = await screen.findByRole('complementary', { name: 'Frame' });
    await within(panel).findByRole('heading', { name: 'f1.fits' });
    fireEvent.click(within(panel).getByRole('button', { name: 'Keep out' }));
    await waitFor(() => expect(seen.choices).toEqual([{ frame_uuids: ['f1'], choice: 'exclude' }]));
    fireEvent.click(within(panel).getByRole('button', { name: 'After only ▸' }));
    await waitFor(() => expect(seen.validity).toEqual([{ frame_uuids: ['f1'], direction: 'forward' }]));
    fireEvent.click(within(panel).getByRole('button', { name: 'Forget' }));
    await waitFor(() => expect(seen.forgotten).toEqual(['f1']));
    await waitFor(() => expect(params().get('calFrame')).toBe('f2'));
  });

  it('keeps a frame out by hand even when its grade already does, with database management off too', async () => {
    const seen = serve();
    const first = renderPage('/calibration?db=demo&calSession=s1&calFrame=f2');
    const panel = await screen.findByRole('complementary', { name: 'Frame' });
    await within(panel).findByRole('heading', { name: 'f2.fits' });
    // Its grade keeps it out: Keep out pins that by hand, Use anyway lets it in.
    expect(within(panel).getByRole('button', { name: 'Use anyway' })).toBeInTheDocument();
    expect(within(panel).queryByRole('button', { name: 'Clear' })).toBeNull();
    fireEvent.click(within(panel).getByRole('button', { name: 'Keep out' }));
    await waitFor(() => expect(seen.choices).toEqual([{ frame_uuids: ['f2'], choice: 'exclude' }]));
    first.unmount();

    // Kept out by hand: Use anyway and Clear remain, Keep out does not.
    serve();
    server.use(http.get('/api/db/demo/calibrations/frames/f2', () => ok(detail('f2', 's1', {
      ...FRAMES.s1[1], quality_override: 'exclude', kept_out: 'by hand',
    }))));
    const second = renderPage('/calibration?db=demo&calSession=s1&calFrame=f2');
    const pinned = await screen.findByRole('complementary', { name: 'Frame' });
    await within(pinned).findByText('Kept out by hand');
    expect(within(pinned).queryByRole('button', { name: 'Keep out' })).toBeNull();
    expect(within(pinned).getByRole('button', { name: 'Use anyway' })).toBeInTheDocument();
    expect(within(pinned).getByRole('button', { name: 'Clear' })).toBeInTheDocument();
    second.unmount();

    // Those are row edits: an editor makes them whether or not the server
    // allows database management.
    serve({ manage: false });
    renderPage('/calibration?db=demo&calSession=s1&calFrame=f2');
    const unmanaged = await screen.findByRole('complementary', { name: 'Frame' });
    await within(unmanaged).findByRole('heading', { name: 'f2.fits' });
    expect(within(unmanaged).getByRole('button', { name: 'Use anyway' })).toBeInTheDocument();
    expect(within(unmanaged).getByRole('button', { name: 'Forget' })).toBeInTheDocument();
    expect(screen.queryByText(/view frames, not change them/)).toBeNull();
  });

  it('uses a suspect frame anyway, after which it reads as OK with no Check', async () => {
    const seen = serve();
    let choice: string | null = null;
    const suspect = () => frame('f3', 's1', {
      grade: 'suspect', grade_code: 'noise', grade_reason: 'noise 1.3 times that of its session',
      quality_override: choice,
    });
    server.use(
      http.get('/api/db/demo/calibrations/frames', () => {
        const frames = [FRAMES.s1[0], FRAMES.s1[1], suspect()];
        return ok({ frames, total: frames.length, page: 0, per_page: 500 });
      }),
      http.get('/api/db/demo/calibrations/frames/f3', () => ok(detail('f3', 's1', {
        ...suspect(),
        findings: [{ code: 'noise', grade: 'suspect', value: 1.3, limit: 1.2, text: 'noise 1.3 times that of its session' }],
      }))),
      http.put('/api/db/demo/calibrations/frames/quality', async ({ request }) => {
        const body = await request.json() as { choice: string };
        seen.choices.push(body);
        choice = body.choice === 'clear' ? null : body.choice;
        return ok({ updated: 1 });
      }),
    );
    renderPage('/calibration?db=demo&calSession=s1&calFrame=f3');
    const panel = await screen.findByRole('complementary', { name: 'Frame' });
    await within(panel).findByRole('heading', { name: 'f3.fits' });
    const thumb = await screen.findByRole('button', { name: 'Frame 3: f3.fits' });
    // Suspect stays in masters but is flagged: a badge here, Check in the strip.
    expect(within(panel).getByText('Suspect · noise')).toBeInTheDocument();
    expect(within(thumb).getByText('Check')).toBeInTheDocument();
    const findings = within(panel).getByRole('list', { name: 'Findings' });
    expect(within(findings).getByText('Suspect')).toBeInTheDocument();
    const use =within(panel).getByRole('button', { name: 'Use anyway' });
    expect(use).toHaveAttribute('title', 'Use anyway: this frame is fine; ignore the suspect grade');
    fireEvent.click(use);
    await waitFor(() => expect(seen.choices).toEqual([{ frame_uuids: ['f3'], choice: 'use' }]));

    // Overruled: Used anyway, no suspect badge, no Check; Clear restores.
    await within(panel).findByText('Used anyway');
    expect(within(panel).queryByText('Suspect · noise')).toBeNull();
    expect(within(panel).getByText('Suspect grade overruled: noise 1.3 times that of its session')).toBeInTheDocument();
    // Its findings stay as plain numbers: no grade word, no grade colour.
    const neutral = within(panel).getByRole('list', { name: 'Findings' });
    expect(within(neutral).queryByText('Suspect')).toBeNull();
    expect(neutral.querySelector('[class*="calibration-finding-"]')).toBeNull();
    expect(within(neutral).getByText(/noise 1\.3 times that of its session · limit/)).toHaveClass('calibration-muted');
    await waitFor(() => expect(within(screen.getByRole('button', { name: 'Frame 3: f3.fits' })).queryByText('Check')).toBeNull());
    expect(within(panel).queryByRole('button', { name: 'Use anyway' })).toBeNull();
    fireEvent.click(within(panel).getByRole('button', { name: 'Clear' }));
    await waitFor(() => expect(seen.choices).toHaveLength(2));
    expect(seen.choices[1]).toEqual({ frame_uuids: ['f3'], choice: 'clear' });
    await within(panel).findByText('Suspect · noise');
  });

  it('offers no writes to a viewer', async () => {
    serve();
    const frames = renderPage('/calibration?db=demo&calSession=s1&calFrame=f2', { canWrite: false });
    const panel = await screen.findByRole('complementary', { name: 'Frame' });
    await within(panel).findByRole('heading', { name: 'f2.fits' });
    expect(await screen.findByText('Your account can view frames, not change them.')).toBeInTheDocument();
    expect(within(panel).queryByRole('button', { name: 'Keep out' })).toBeNull();
    expect(within(panel).queryByRole('button', { name: 'Use anyway' })).toBeNull();
    expect(within(panel).queryByRole('button', { name: 'Forget' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Forget session' })).toBeNull();
    frames.unmount();

    serve();
    renderPage('/calibration?db=demo&calTab=masters', { canWrite: false });
    const masters = await screen.findByRole('region', { name: 'Masters' });
    await within(masters).findByText('Used by Heart');
    expect(screen.queryByRole('button', { name: /Clear/ })).toBeNull();
  });

  it('lists masters and opens one in the inspector from the URL', async () => {
    serve();
    renderPage('/calibration?db=demo&calTab=masters');
    const masters = await screen.findByRole('region', { name: 'Masters' });
    expect(await within(masters).findByText('Used by Heart')).toBeInTheDocument();
    expect(within(masters).getByText('Too few frames')).toBeInTheDocument();
    expect(within(masters).getByText('Poor · spots')).toBeInTheDocument();
    expect(within(masters).getByText('31 changing, 25 carried, 4 steady spots')).toBeInTheDocument();
    // Built by the server: on the viewer's clock, with its zone.
    expect(await within(masters).findByText(`Askar · ASI2600 · built ${serverTime(1757600000)}`)).toBeInTheDocument();
    fireEvent.click(within(masters).getByRole('button', { name: /View Flat master/ }));
    await waitFor(() => expect(params().get('calMaster')).toBe('m1'));
    const inspector = await screen.findByRole('dialog');
    expect(within(inspector).getByText('Calibration library')).toBeInTheDocument();
    const image = await within(inspector).findByTestId('stack-inspector-image');
    expect(new URL((image as HTMLImageElement).src).pathname).toBe('/api/db/demo/calibrations/masters/m1/preview');
    fireEvent.click(within(inspector).getByRole('button', { name: 'Close calibration master inspector' }));
    await waitFor(() => expect(params().get('calMaster')).toBeNull());
  });

  it('moves the Scheduler flats tab into the page', async () => {
    serve();
    server.use(http.get('/api/db/demo/flat-history', () => ok({ records: [], total: 0, limit: 50, offset: 0, pending: 0 })));
    renderPage('/calibration?db=demo&calTab=scheduler');
    expect(await screen.findByRole('tabpanel', { name: 'Scheduler flats' })).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'Scheduler flats' })).toHaveAttribute('aria-selected', 'true');
  });

  describe('duplicates', () => {
    const report = (token: string, groups = DUPLICATE_GROUPS) => ({
      groups,
      rows_removed: groups.length,
      missing_rows: groups.filter((group) => group.members.some((member) => !member.exists)).length,
      physical_rows: groups.filter((group) => group.physical).length,
      repointed: 0,
      masters: groups.length ? [{ master_uuid: 'dc6ba609-0000', kind: 'bias', source_count: 12, created_at: 1757600000, direct: true, doubled: true }] : [],
      token,
    });
    const IDLE = {
      state: 'idle', action: null, stage: null, done: 0, total: 0, run: 0, started_at: null, finished_at: null,
      report: null, checked_at: null, repaired: null, changed: null, error: null,
    };
    // The server's polls are a second apart.
    const POLL = { timeout: 4000 };

    /**
     * The duplicates job as the server runs it: a POST starts a run, which
     * the next status read finishes unless `hold` keeps it going. `changed`
     * makes the first repair find the plan moved on. `lateRead` holds back
     * each read of the finished repair, so it can answer after the next run
     * started.
     */
    function serveDuplicates({ manage = true, changed = false, stale = false, lateRead = false }: { manage?: boolean; changed?: boolean; stale?: boolean; lateRead?: boolean } = {}) {
      serve({ manage });
      const seen = { checks: 0, repairs: [] as unknown[], stops: 0, automatic: [] as boolean[] };
      const job = { hold: false };
      // `stale`: the server kept a plan from before a sync changed the rows.
      let status: Record<string, unknown> = stale
        ? { ...IDLE, state: 'done', action: 'check', run: 1, report: report('t0'), checked_at: Date.now() / 1000 - 3600, stale: true }
        : { ...IDLE };
      let finish: (() => void) | null = null;
      let applied = false;
      let plans = 0;
      const plan = () => {
        plans += 1;
        return applied ? report(`t${plans}`, []) : report(`t${plans}`);
      };
      // The words the server sends with each stage.
      const verbs: Record<string, string> = {
        checking_files: 'Checking files', comparing_copies: 'Comparing copies', merging: 'Merging',
      };
      const begin = (action: 'check' | 'repair', stage: string, total: number, then: () => void) => {
        const done = Math.floor(total / 2);
        status = {
          ...status, state: 'running', action, stage, done, total,
          title: action === 'repair' ? 'Repairing duplicates' : 'Checking for duplicates',
          label: `${verbs[stage]} ${done.toLocaleString('en-US')} of ${total.toLocaleString('en-US')}`,
          percent: (100 * done) / total,
          run: (status.run as number) + 1, changed: null, error: null,
        };
        finish = then;
      };
      server.use(
        http.get('/api/db/demo/calibrations/duplicates', async () => {
          if (finish && !job.hold) {
            const then = finish;
            finish = null;
            then();
            return ok(status);
          }
          const read = status;
          if (lateRead && read.action === 'repair' && read.state === 'done') await delay(300);
          return ok(read);
        }),
        http.post('/api/db/demo/calibrations/duplicates/check', ({ request }) => {
          seen.checks += 1;
          seen.automatic.push(new URL(request.url).searchParams.get('automatic') === 'true');
          if (status.state !== 'running') {
            begin('check', 'checking_files', 2140, () => {
              status = { ...status, state: 'done', stage: null, done: 2140, report: plan(), checked_at: Date.now() / 1000 - 120, stale: false };
            });
          }
          return HttpResponse.json({ success: true, data: status }, { status: 202 });
        }),
        http.post('/api/db/demo/calibrations/duplicates/repair', async ({ request }) => {
          seen.repairs.push(await request.json());
          begin('repair', 'merging', 2, () => {
            if (changed && seen.repairs.length === 1) {
              status = {
                ...status, state: 'done', stage: null, report: plan(),
                changed: {
                  groups_before: 2, groups_after: 2, rows_before: 2, rows_after: 2,
                  added: [], added_count: 0, dropped: [], dropped_count: 0,
                  altered: ['dark_0003.fits'], altered_count: 1,
                },
              };
              return;
            }
            applied = true;
            status = {
              ...status, state: 'done', stage: null, done: 2, report: null, checked_at: null,
              repaired: { run: status.run, finished_at: 1757600100, outcome: { groups: 2, rows_removed: 2, repointed: 0, masters_removed: 1 } },
            };
          });
          return HttpResponse.json({ success: true, data: status }, { status: 202 });
        }),
        http.post('/api/db/demo/calibrations/duplicates/stop', () => {
          seen.stops += 1;
          finish = null;
          status = { ...status, state: 'stopped', stage: null, report: status.action === 'check' ? null : status.report };
          return ok(status);
        }),
      );
      return { seen, job };
    }

    it('checks the library on arrival, with its progress, then lists each exposure recorded twice', async () => {
      const { seen, job } = serveDuplicates();
      job.hold = true;
      renderPage('/calibration?db=demo&calDupes=true');
      const section = await screen.findByRole('region', { name: 'Duplicates' });
      const progress = await within(section).findByRole('progressbar', { name: 'Checking for duplicates' });
      expect(progress).toHaveAttribute('aria-valuetext', 'Checking files 1,070 of 2,140');
      expect(within(section).getByText(/Checking files 1,070 of 2,140/)).toBeInTheDocument();
      expect(seen.checks).toBe(1);
      job.hold = false;
      expect(await within(section).findByText('2 exposures recorded more than once', {}, POLL)).toBeInTheDocument();
      expect(within(section).queryByRole('progressbar')).toBeNull();
      const flat = within(section).getByRole('listitem', { name: 'flat_0007.fits' });
      expect(within(flat).getByText('Keep')).toBeInTheDocument();
      expect(within(flat).getByText('Missing')).toBeInTheDocument();
      expect(within(flat).getByText('/data/_Incoming/flat_0007.fits')).toBeInTheDocument();
      // When it was shot, on its rig's clock, as the frame strip reads it.
      expect(await within(flat).findByText(/^2025-09-10 21:00 UTC−7 · Askar · ASI2600/)).toBeInTheDocument();
      expect(screen.getByRole('checkbox', { name: 'Duplicates' })).toBeChecked();
      // Only the filters this view reads are offered.
      expect(screen.queryByLabelText('Grade')).toBeNull();
      expect(screen.queryByRole('checkbox', { name: 'Missing files' })).toBeNull();
      // The type filter narrows the groups shown.
      fireEvent.change(screen.getByLabelText('Type'), { target: { value: 'dark' } });
      await waitFor(() => expect(within(section).getByText('1 of 2 exposures recorded more than once')).toBeInTheDocument());
      expect(seen.checks).toBe(1);
      // Turning the filter off goes back to sessions.
      fireEvent.click(screen.getByRole('checkbox', { name: 'Duplicates' }));
      await waitFor(() => expect(params().get('calDupes')).toBeNull());
      expect(await screen.findByRole('navigation', { name: 'Sessions' })).toBeInTheDocument();
    });

    it('previews the repair, applies it with its progress, and shows what it did', async () => {
      const { seen, job } = serveDuplicates();
      renderPage('/calibration?db=demo&calDupes=true');
      fireEvent.click(await screen.findByRole('button', { name: 'Repair duplicates…' }, POLL));
      const dialog = await screen.findByRole('dialog', { name: 'Repair duplicates' });
      const preview = within(dialog).getByRole('region', { name: 'Repair preview' });
      expect(await within(preview).findByText('2 exposures recorded more than once', {}, POLL)).toBeInTheDocument();
      expect(seen.checks).toBe(2);
      expect(within(preview).getByText(/1 master to rebuild; 1 included an exposure twice/)).toBeInTheDocument();
      expect(within(preview).getByText(/covers all rigs and frame types in this library/)).toBeInTheDocument();

      job.hold = true;
      fireEvent.click(within(dialog).getByRole('button', { name: 'Apply' }));
      const merging = await within(dialog).findByRole('progressbar', { name: 'Repairing duplicates' });
      expect(merging).toHaveAttribute('aria-valuetext', 'Merging 1 of 2');
      expect(within(dialog).getByRole('button', { name: 'Stop' })).toBeInTheDocument();
      expect(seen.repairs).toEqual([{ token: 't2' }]);
      job.hold = false;
      expect(await screen.findByText(/Repaired 2 exposures: 2 rows merged, 1 master to build again/, {}, POLL)).toBeInTheDocument();
      expect(screen.queryByRole('dialog')).toBeNull();
      // The plan is spent, so the view checks again and finds nothing.
      expect(await screen.findByText('No exposure is recorded more than once.', {}, POLL)).toBeInTheDocument();
    });

    // CI: the repair refreshes the calibration queries, and that read of
    // the finished repair landed after the check that follows it started,
    // which stopped the polling on "Checking the library…".
    it('follows the check after a repair when an older status read lands late', async () => {
      const { seen } = serveDuplicates({ lateRead: true });
      renderPage('/calibration?db=demo&calDupes=true');
      fireEvent.click(await screen.findByRole('button', { name: 'Repair duplicates…' }, POLL));
      const dialog = await screen.findByRole('dialog', { name: 'Repair duplicates' });
      fireEvent.click(await within(dialog).findByRole('button', { name: 'Apply' }, POLL));
      expect(await screen.findByText(/Repaired 2 exposures/, {}, POLL)).toBeInTheDocument();
      expect(await screen.findByText('No exposure is recorded more than once.', {}, POLL)).toBeInTheDocument();
      expect(seen.checks).toBe(3);
    });

    it('says what changed when the plan moved on, and applies the new plan', async () => {
      const { seen } = serveDuplicates({ changed: true });
      renderPage('/calibration?db=demo&calDupes=true');
      fireEvent.click(await screen.findByRole('button', { name: 'Repair duplicates…' }, POLL));
      const dialog = await screen.findByRole('dialog', { name: 'Repair duplicates' });
      fireEvent.click(await within(dialog).findByRole('button', { name: 'Apply' }, POLL));
      const alert = await within(dialog).findByRole('alert', {}, POLL);
      expect(alert).toHaveTextContent('The plan changed since the preview, so nothing was applied.');
      expect(alert).toHaveTextContent('1 exposure whose rows or files changed: dark_0003.fits');
      fireEvent.click(within(dialog).getByRole('button', { name: 'Apply' }));
      await waitFor(() => expect(seen.repairs).toEqual([{ token: 't2' }, { token: 't3' }]));
      expect(await screen.findByText(/Repaired 2 exposures/, {}, POLL)).toBeInTheDocument();
    });

    it('stops a check, and checks again when asked', async () => {
      const { seen, job } = serveDuplicates();
      job.hold = true;
      renderPage('/calibration?db=demo&calDupes=true');
      const section = await screen.findByRole('region', { name: 'Duplicates' });
      fireEvent.click(await within(section).findByRole('button', { name: 'Stop' }));
      expect(await within(section).findByText('The check for duplicates was stopped.')).toBeInTheDocument();
      expect(seen.stops).toBe(1);
      expect(seen.checks).toBe(1);
      job.hold = false;
      fireEvent.click(within(section).getByRole('button', { name: 'Check again' }));
      expect(await within(section).findByText('2 exposures recorded more than once', {}, POLL)).toBeInTheDocument();
      expect(seen.checks).toBe(2);
    });

    it('keeps the repair behind database management', async () => {
      serveDuplicates({ manage: false });
      renderPage('/calibration?db=demo&calDupes=true');
      expect(await screen.findByText(/needs database management/, {}, POLL)).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Repair duplicates…' })).toBeNull();
    });

    it('checks in the background on arrival, and at once when asked', async () => {
      const { seen } = serveDuplicates();
      renderPage('/calibration?db=demo&calDupes=true');
      const section = await screen.findByRole('region', { name: 'Duplicates' });
      expect(await within(section).findByText('2 exposures recorded more than once', {}, POLL)).toBeInTheDocument();
      expect(seen.automatic).toEqual([true]);
      expect(within(section).getByText('Checked 2 min ago')).toBeInTheDocument();
      fireEvent.click(within(section).getByRole('button', { name: 'Check again' }));
      await waitFor(() => expect(seen.automatic).toEqual([true, false]));
    });

    it('keeps a plan the catalog has not moved on from, and checks again one it has', async () => {
      const { seen } = serveDuplicates({ stale: true });
      renderPage('/calibration?db=demo&calDupes=true');
      const section = await screen.findByRole('region', { name: 'Duplicates' });
      // The old plan shows while a background check makes the new one.
      expect(await within(section).findByRole('listitem', { name: 'flat_0007.fits' })).toBeInTheDocument();
      await waitFor(() => expect(seen.automatic).toEqual([true]));
      expect(await within(section).findByText('Checked 2 min ago', {}, POLL)).toBeInTheDocument();
      // The fresh plan is not stale: no further check.
      await new Promise((resolve) => setTimeout(resolve, 300));
      expect(seen.checks).toBe(1);
    });

    // REVIEW #877: a read-only account without compute gets 403 from
    // POST …/duplicates/check (auth.rs requires_write). Before the PR the
    // GET computed the list, so such an account saw the duplicates.
    it('does not leave a read-only viewer on "Checking the library…" forever', async () => {
      serveDuplicates({ manage: false });
      server.use(
        http.post('/api/db/demo/calibrations/duplicates/check', () =>
          HttpResponse.json({ success: false, data: null, error: 'This account has read-only access' }, { status: 403 })),
      );
      renderPage('/calibration?db=demo&calDupes=true');
      const section = await screen.findByRole('region', { name: 'Duplicates' });
      await waitFor(() => expect(within(section).getByRole('alert')).toBeInTheDocument(), POLL);
      expect(within(section).queryByText('Checking the library…')).toBeNull();
    });
  });
});

const DUPLICATE_GROUPS = [
  {
    kind: 'flat', captured_at: 1757563200, file_name: 'flat_0007.fits', rig_uuid: 'rig', rig_name: 'Askar · ASI2600',
    keep: 'k1', path: '/data/_Calibration/flat_0007.fits', repointed: false, physical: false,
    members: [
      { frame_uuid: 'k1', source_path: '/data/_Calibration/flat_0007.fits', exists: true, keep: true },
      { frame_uuid: 'r1', source_path: '/data/_Incoming/flat_0007.fits', exists: false, keep: false },
    ],
  },
  {
    kind: 'dark', captured_at: 1757500000, file_name: 'dark_0003.fits', rig_uuid: 'rig', rig_name: 'Askar · ASI2600',
    keep: 'k2', path: '/data/darks/dark_0003.fits', repointed: false, physical: true,
    members: [
      { frame_uuid: 'k2', source_path: '/data/darks/dark_0003.fits', exists: true, keep: true },
      { frame_uuid: 'r2', source_path: '/data/darks-copy/dark_0003.fits', exists: true, keep: false },
    ],
  },
];
