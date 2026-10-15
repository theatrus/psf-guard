import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { configure, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { MemoryRouter, Route, Routes, useLocation, useSearchParams } from 'react-router-dom';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { server } from '../../../test/msw-server';
import CalibrationPage from '../CalibrationPage';
import { listFilters } from '../useCalibrationUrlState';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

const RIGS = [
  { rig_uuid: 'askar', name: 'Askar 120', profile_ids: ['p1'], frame_count: 5, bias: 0, dark: 2, dark_flat: 0, flat: 3 },
  { rig_uuid: 'c925', name: 'C925', profile_ids: ['p2'], frame_count: 4, bias: 0, dark: 2, dark_flat: 0, flat: 2 },
];

function session(id: string, rig: string) {
  return {
    id, kind: 'flat', night: '2026-09-10', rig_uuid: rig, rig_name: rig, is_master: false,
    filter: 'Ha', exposure_min_s: 2, exposure_max_s: 2, temp_min_c: -10, temp_max_c: -10,
    gain: 100, offset: 20, readout: null, binning: '1x1', width: 100, height: 100, rotation: null,
    first_at: 1757563200, last_at: 1757563800, frames: 3, ok: 3, suspect: 0, poor: 0, ungraded: 0, kept_out: 0,
  };
}

function project(id: number, profile: string, name: string) {
  return { id, profile_id: profile, profile_name: profile, name, display_name: name, description: null, has_files: true, state: 1, latest_image_date: null };
}

interface Seen {
  sessions: URLSearchParams[];
  masters: URLSearchParams[];
  groups: URLSearchParams[];
}

/** Demo holds M 44 (7) and Heart (8); M 44's lights use the Askar only,
 *  unless `spans` says otherwise, and Heart's the C925. Other holds Iris (9). */
function serve({ spans = ['askar'] }: { spans?: string[] } = {}): Seen {
  const seen: Seen = { sessions: [], masters: [], groups: [] };
  const database = (id: string, name: string) =>
    ({ id, name, database_path: `/${id}.sqlite`, image_directories: [], remote_image_upload: { enabled: false } });
  server.use(
    http.get('/api/info', () => ok({ version: 'test', cache_directory: '/cache', allow_database_management: true })),
    http.get('/api/databases', () => ok([database('demo', 'Demo'), database('other', 'Other'), database('empty', 'Empty')])),
    http.get('/api/db/:db/calibrations', () => ok({ schema_version: 9, frame_count: 9, master_count: 1, rigs: RIGS })),
    http.get('/api/db/demo/projects', () => ok([project(7, 'p1', 'M 44'), project(8, 'p2', 'Heart')])),
    http.get('/api/db/other/projects', () => ok([project(9, 'p1', 'Iris')])),
    http.get('/api/db/empty/projects', () => ok([])),
    http.get('/api/db/:db/calibrations/project-scope', ({ request }) => {
      const id = Number(new URL(request.url).searchParams.get('project'));
      const rigs = id === 8 ? ['c925'] : spans;
      return ok({
        project_id: id, lights: 40, unread: 0, sessions: ['s1'], frames: 3, masters: ['m1'],
        rigs: rigs.map((rig) => ({ rig_uuid: rig, name: RIGS.find((entry) => entry.rig_uuid === rig)?.name ?? rig })),
        pending: 0, revision: `r${id}`, refreshing: false,
      });
    }),
    http.get('/api/db/:db/calibrations/sessions', ({ request }) => {
      const params = new URL(request.url).searchParams;
      seen.sessions.push(params);
      const sessions = params.get('project') ? [session('s1', 'askar')] : [session('s1', 'askar'), session('s2', 'askar')];
      return ok({
        sessions, total: sessions.length, page: 0, per_page: 50, frames: sessions.length * 3,
        facets: { filters: ['Ha'], exposures: [2], first_night: '2026-09-10', last_night: '2026-09-10', rotations: [] },
        night_boundary_seconds: 43200,
      });
    }),
    http.get('/api/db/:db/calibrations/frames', () => ok({ frames: [], total: 0, page: 0, per_page: 500 })),
    http.get('/api/db/:db/calibrations/flat-groups', ({ request }) => {
      seen.groups.push(new URL(request.url).searchParams);
      return ok({ available: true, rigs: [] });
    }),
    http.get('/api/db/:db/calibrations/quality', () => ok({ progress: { running: false, stage: null, finished_at: null } })),
    http.get('/api/db/:db/calibrations/masters', ({ request }) => {
      seen.masters.push(new URL(request.url).searchParams);
      return ok({ masters: [], notes: [] });
    }),
    http.post('/api/db/:db/calibrations/generation-status', () => ok({ statuses: [] })),
  );
  return seen;
}

function Location() {
  const location = useLocation();
  return <output aria-label="location">{location.search}</output>;
}

/** What the header does: another project, maybe another rig's database. */
function Header() {
  const [, setSearchParams] = useSearchParams();
  const open = (db: string, project: string | null) =>
    setSearchParams((previous) => {
      const next = new URLSearchParams(previous);
      next.set('db', db);
      if (project) next.set('project', project); else next.delete('project');
      return next;
    }, { replace: true });
  return (
    <>
      <button type="button" onClick={() => open('demo', '8')}>Open Heart</button>
      <button type="button" onClick={() => open('other', '9')}>Switch to Other</button>
      <button type="button" onClick={() => open('empty', null)}>Switch to Empty</button>
    </>
  );
}

function renderPage(entry: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[entry]}>
        <Routes>
          <Route path="/calibration" element={<>{children}<Location /><Header /></>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return render(<CalibrationPage />, { wrapper });
}

const params = () => new URLSearchParams(screen.getByRole('status', { name: 'location' }).textContent ?? '');
const chip = () => screen.queryByRole('group', { name: /^Scope: / });
/** The heading's link, which does what the chip's ✕ does. */
const headingLink = () => {
  const link = screen.getAllByRole('button', { name: /^Show all of .*'s calibration$/ })
    .find((button) => button.classList.contains('calibration-link-button'));
  expect(link).toBeTruthy();
  return link!;
};

describe('the Calibration page scoped to a project', () => {
  // Each step waits on two or three requests and the effects after them;
  // a busy full run can take longer than the default second.
  beforeAll(() => configure({ asyncUtilTimeout: 10_000 }));
  afterAll(() => configure({ asyncUtilTimeout: 1_000 }));

  it('starts on the calibration the project uses, on its rig, and says so', async () => {
    const seen = serve();
    renderPage('/calibration?db=demo&project=7&calProject=7');
    expect(await screen.findByRole('group', { name: 'Scope: M 44 on Demo' }, { timeout: 10_000 })).toHaveClass('is-set');
    // The one rig its lights use is the Rig filter's choice.
    await waitFor(() => expect(params().get('calRig')).toBe('askar'), { timeout: 10_000 });
    await waitFor(() => expect(seen.sessions.at(-1)?.get('project')).toBe('7'));
    expect(seen.sessions.at(-1)?.get('rig')).toBe('askar');
    expect(seen.groups.at(-1)?.get('project')).toBe('7');
    expect(await screen.findByTestId('calibration-scope-heading')).toHaveTextContent('· for M 44 · Askar 120');
    expect(screen.getByText(/What its 40 lights use: 1 session, 1 master/)).toBeInTheDocument();
    // The chip comes first in the filter bar, before Filters and the Rig pill.
    const bar = screen.getByRole('region', { name: 'Calibration filters' });
    expect(bar.querySelector('.view-toolbar-bar')?.firstElementChild).toContainElement(chip());

    // The Masters tab keeps the scope.
    fireEvent.click(screen.getByRole('tab', { name: 'Masters' }));
    await waitFor(() => expect(seen.masters.at(-1)?.get('project')).toBe('7'));
    expect(screen.getByRole('group', { name: 'Scope: M 44 on Demo' })).toBeInTheDocument();
  });

  it('starts on the whole library without a project scope', async () => {
    const seen = serve();
    renderPage('/calibration?db=demo&project=7');
    await waitFor(() => expect(seen.sessions.length).toBeGreaterThan(0), { timeout: 10_000 });
    expect(chip()).not.toBeInTheDocument();
    expect(screen.queryByTestId('calibration-scope-heading')).not.toBeInTheDocument();
    expect(seen.sessions.every((query) => !query.has('project'))).toBe(true);
  });

  it('widens to the rig\'s whole library with ✕, keeping the Rig filter', async () => {
    const seen = serve();
    renderPage('/calibration?db=demo&project=7&calProject=7');
    await waitFor(() => expect(params().get('calRig')).toBe('askar'), { timeout: 10_000 });
    fireEvent.click(within(chip()!).getByRole('button', { name: "Show all of Demo's calibration" }));
    await waitFor(() => expect(params().has('calProject')).toBe(false));
    expect(params().get('calRig')).toBe('askar');
    await waitFor(() => expect(seen.sessions.at(-1)?.has('project')).toBe(false));
    expect(seen.sessions.at(-1)?.get('rig')).toBe('askar');
    expect(chip()).not.toBeInTheDocument();
    // The Rig filter's own choice then gives the whole catalog.
    fireEvent.change(screen.getByLabelText('Rig'), { target: { value: 'any' } });
    await waitFor(() => expect(seen.sessions.at(-1)?.has('rig')).toBe(false));
  });

  it('widens the same way from the heading\'s link', async () => {
    serve();
    renderPage('/calibration?db=demo&project=7&calProject=7');
    await waitFor(() => expect(params().get('calRig')).toBe('askar'), { timeout: 10_000 });
    fireEvent.click(headingLink());
    await waitFor(() => expect(params().has('calProject')).toBe(false));
    expect(params().get('calRig')).toBe('askar');
    expect(screen.queryByTestId('calibration-scope-heading')).not.toBeInTheDocument();
  });

  it('clears the scope with every filter', async () => {
    serve();
    renderPage('/calibration?db=demo&project=7&calProject=7&calKind=flat');
    await waitFor(() => expect(params().get('calRig')).toBe('askar'), { timeout: 10_000 });
    fireEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
    await waitFor(() => expect(params().has('calProject')).toBe(false));
    expect(params().has('calRig')).toBe(false);
    expect(params().has('calKind')).toBe(false);
  });

  it('reads the scope from the address, and writes it to the lists', async () => {
    const seen = serve();
    // The header's project is Heart; the page's scope is still M 44.
    renderPage('/calibration?db=demo&project=8&calProject=7&calRig=c925');
    expect(await screen.findByRole('group', { name: 'Scope: M 44 on Demo' }, { timeout: 10_000 })).toBeInTheDocument();
    await waitFor(() => expect(seen.sessions.at(-1)?.get('project')).toBe('7'));
    // A rig chosen in the address stays.
    expect(params().get('calRig')).toBe('c925');
    expect(listFilters({ project: 7, rig: 'any' })).toEqual({ project: 7 });
  });

  it('shows every rig a project spans, with All rigs', async () => {
    const seen = serve({ spans: ['askar', 'c925'] });
    renderPage('/calibration?db=demo&project=7&calProject=7');
    expect(await screen.findByRole('group', { name: 'Scope: M 44 on Demo · 2 rigs' }, { timeout: 10_000 })).toBeInTheDocument();
    await waitFor(() => expect(params().get('calRig')).toBe('any'));
    await waitFor(() => expect(seen.sessions.at(-1)?.has('rig')).toBe(false));
    expect(screen.getByTestId('calibration-scope-heading')).toHaveTextContent('· for M 44 · 2 rigs');
  });

  it('follows the project to another rig, and widens where the rig has none', async () => {
    serve();
    renderPage('/calibration?db=demo&project=7&calProject=7');
    await screen.findByRole('group', { name: 'Scope: M 44 on Demo' }, { timeout: 10_000 });
    fireEvent.click(screen.getByRole('button', { name: 'Switch to Other' }));
    await waitFor(() => expect(params().get('calProject')).toBe('9'));
    expect(await screen.findByRole('group', { name: 'Scope: Iris on Other' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Switch to Empty' }));
    await waitFor(() => expect(params().get('db')).toBe('empty'));
    await waitFor(() => expect(params().has('calProject')).toBe(false));
    expect(chip()).not.toBeInTheDocument();
  });

  it("opens the next project on its own rig, not the last project's", async () => {
    const seen = serve();
    renderPage('/calibration?db=demo&project=7&calProject=7');
    await waitFor(() => expect(params().get('calRig')).toBe('askar'), { timeout: 10_000 });
    // Heart's lights use the C925 only.
    fireEvent.click(screen.getByRole('button', { name: 'Open Heart' }));
    await waitFor(() => expect(params().get('calProject')).toBe('8'));
    await waitFor(() => expect(params().get('calRig')).toBe('c925'), { timeout: 10_000 });
    await waitFor(() => expect(seen.sessions.at(-1)?.get('rig')).toBe('c925'));
    expect(seen.sessions.at(-1)?.get('project')).toBe('8');
  });

  it('follows another project chosen in the header, and stays wide once widened', async () => {
    serve();
    renderPage('/calibration?db=demo&project=7&calProject=7');
    await screen.findByRole('group', { name: 'Scope: M 44 on Demo' }, { timeout: 10_000 });
    fireEvent.click(screen.getByRole('button', { name: 'Open Heart' }));
    await waitFor(() => expect(params().get('calProject')).toBe('8'));
    fireEvent.click(headingLink());
    await waitFor(() => expect(params().has('calProject')).toBe(false));
    fireEvent.click(screen.getByRole('button', { name: 'Switch to Other' }));
    await waitFor(() => expect(params().get('db')).toBe('other'));
    expect(params().has('calProject')).toBe(false);
  });
});
