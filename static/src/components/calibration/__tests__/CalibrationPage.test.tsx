import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { delay, http, HttpResponse } from 'msw';
import { MemoryRouter, Route, Routes, useLocation, useSearchParams } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { apiClient } from '../../../api/client';
import { AccessContext, useAccess } from '../../../auth/access';
import { server } from '../../../test/msw-server';
import CalibrationPage from '../CalibrationPage';

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
      rigs: [{ rig_uuid: 'rig', name: 'Askar · ASI2600', frame_count: 5, bias: 0, dark: 2, dark_flat: 0, flat: 3 }],
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
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <WithAccess canWrite={canWrite}>
        <MemoryRouter initialEntries={[entry]}>
          <Routes>
            <Route path="/calibration" element={<>{children}<Location /><Scope /></>} />
          </Routes>
        </MemoryRouter>
      </WithAccess>
    </QueryClientProvider>
  );
  return render(<CalibrationPage />, { wrapper });
}

const params = () => new URLSearchParams(screen.getByRole('status', { name: 'location' }).textContent ?? '');

afterEach(() => vi.restoreAllMocks());

describe('CalibrationPage', () => {
  it('opens on the first session and its first frame, and keeps both in the URL', async () => {
    serve();
    renderPage('/calibration?db=demo');
    await waitFor(() => expect(params().get('calSession')).toBe('s1'));
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
    renderPage('/calibration?db=demo&project=7');
    await waitFor(() => expect(params().get('calRig')).toBe('rig2'));
    fireEvent.click(screen.getByRole('button', { name: 'Open Iris' }));
    await waitFor(() => expect(params().get('db')).toBe('other'));
    await waitFor(() => expect(params().get('calRig')).toBeNull());
    await waitFor(() => expect(seen.other.at(-1)?.has('rig')).toBe(false));
    // One rig: no Rig choice, and nothing hidden left choosing.
    expect(screen.queryByLabelText('Rig')).toBeNull();
    // Back in the first database, the project opens on its rig again.
    fireEvent.click(screen.getByRole('button', { name: 'Open Bubble' }));
    await waitFor(() => expect(params().get('db')).toBe('demo'));
    await waitFor(() => expect(params().get('calRig')).toBe('rig2'));
  });

  it('writes filters to the URL and asks the server for them', async () => {
    const seen = serve();
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
    await waitFor(() => expect(seen.sessions.at(-1)?.get('temp_min')).toBe('-12'));
    expect(seen.sessions.at(-1)?.get('grade')).toBe('poor');
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
    expect(within(masters).getByText('Thin')).toBeInTheDocument();
    expect(within(masters).getByText('Poor · spots')).toBeInTheDocument();
    expect(within(masters).getByText('31 changing, 25 carried, 4 steady spots')).toBeInTheDocument();
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
});
