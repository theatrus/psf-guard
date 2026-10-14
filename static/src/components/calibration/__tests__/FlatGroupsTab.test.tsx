import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import type { FlatGroupsRig, FlatGroupsView, FlatSession } from '../../../api/types';
import { server } from '../../../test/msw-server';
import CalibrationPage from '../CalibrationPage';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null });

function session(key: string, night: string, previous: FlatSession['previous'] = null): FlatSession {
  return {
    response_key: key, night, first: 1757563200, last: 1757563800, frames: 10, combined: 10, noise_pct: 0.05,
    bin: 8, width: 780, height: 520, previous,
  };
}

const spots = [
  { kind: 'new' as const, darker: true, x: 1418, y: 3540, radius: 40, peak_pct: 1.6 },
  { kind: 'gone' as const, darker: false, x: 3000, y: 1200, radius: 30, peak_pct: 0.4 },
];

const RIG: FlatGroupsRig = {
  rig_uuid: 'rig', name: 'UltraCat · ASI6200', measured: true,
  dust_limit_pct: 1, pattern_limit_pct: 2,
  angle_term: { dust_pct: 0.3, broad_pct: 0.7 }, angle_pairs: 12,
  events: [{
    key: 'e1', start: 1758184200, end: 1758457680, what: 'new dust: 2 shadows near (1418, 3540)',
    dust_pct: 1.6, broad_pct: 0.2, camera_side: true, filters: ['R', 'G'], rotations: [3, 54],
    spots, before_response: 'r-0916', after_response: 'r-0920',
  }],
  families: [{
    key: 'R:3', filter: 'R', rotation: 3,
    groups: [
      { first_night: '2026-09-20', last_night: '2026-09-20', frames: 10, sessions: [session('r-0920', '2026-09-20')] },
      {
        first_night: '2026-09-07', last_night: '2026-09-16', frames: 20,
        sessions: [
          session('r-0916', '2026-09-16', { dust_pct: 0.09, broad_pct: 0.01 }),
          session('r-0907', '2026-09-07'),
        ],
      },
    ],
    splits: [{
      before_response: 'r-0916', after_response: 'r-0920', before_night: '2026-09-16', after_night: '2026-09-20',
      start: 1758184200, end: 1758457680, dust_pct: 1.6, broad_pct: 0.2,
      reason: 'new dust: 1.6% against a limit of 1.0%', event_key: 'e1', spots,
    }],
  }, {
    key: 'G:54', filter: 'G', rotation: 54,
    groups: [{ first_night: '2026-09-12', last_night: '2026-09-12', frames: 10, sessions: [session('g-0912', '2026-09-12')] }],
    splits: [],
  }],
  unmeasured: [{ night: '2026-09-14', filter: 'R', rotation: 3, frames: 2, reason: 'needs 3 flats' }],
};

const VIEW: FlatGroupsView = { available: true, rigs: [RIG] };

function serve(view: FlatGroupsView = VIEW, progress: Record<string, unknown> = {}) {
  const seen = { groups: 0 };
  server.use(
    http.get('/api/info', () => ok({ version: 'test', cache_directory: '/cache', allow_database_management: true })),
    http.get('/api/databases', () => ok([
      { id: 'demo', name: 'Demo', database_path: '/demo.sqlite', image_directories: [], remote_image_upload: { enabled: false } },
    ])),
    http.get('/api/db/demo/calibrations', () => ok({
      schema_version: 9, frame_count: 30, master_count: 0,
      rigs: [{ rig_uuid: 'rig', name: 'UltraCat · ASI6200', frame_count: 30, bias: 0, dark: 0, dark_flat: 0, flat: 30 }],
    })),
    http.get('/api/db/demo/calibrations/flat-groups', () => {
      seen.groups += 1;
      return ok(view);
    }),
    http.get('/api/db/demo/calibrations/quality', () => ok({
      progress: {
        running: false, stage: null, to_measure: 0, done: 0, workers: 0, started_at: null, finished_at: null,
        stage_done: 0, stage_total: 0, last_flat_groups: null, ...progress,
      },
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
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return render(<CalibrationPage />, { wrapper });
}

const params = () => new URLSearchParams(screen.getByRole('status', { name: 'location' }).textContent ?? '');

describe('Flat groups', () => {
  it('lists the changes, groups, splits and unmeasured nights of each rig', async () => {
    serve();
    renderPage('/calibration?db=demo&calTab=groups');
    const tab = screen.getByRole('tab', { name: 'Flat groups' });
    expect(tab).toHaveAttribute('aria-selected', 'true');
    const panel = await screen.findByRole('tabpanel', { name: 'Flat groups' });
    const rig = await within(panel).findByRole('region', { name: 'UltraCat · ASI6200' });
    expect(within(rig).getByText('Splits at 1% dust, 2% light pattern')).toBeInTheDocument();
    expect(within(rig).getByText(/differ by up to 0.70% pattern, 0.30% dust \(12 pairs\)/)).toBeInTheDocument();

    const events = within(rig).getByTestId('flat-change-events');
    const event = within(events).getByTestId('flat-change-event');
    expect(event).toHaveTextContent('2025-09-18 08:30 – 2025-09-21 12:28 UTC');
    expect(event).toHaveTextContent('new dust: 2 shadows near (1418, 3540)');
    expect(event).toHaveTextContent('dust 1.6% · pattern 0.20% · seen in R, G at 3°, 54°');
    expect(event).toHaveTextContent('All filters and angles: camera side');

    const families = within(rig).getAllByTestId('flat-family');
    expect(families.map((family) => within(family).getByRole('heading').textContent)).toEqual(['R · 3°', 'G · 54°']);
    const groups = within(families[0]).getAllByTestId('flat-group');
    expect(groups.map((group) => group.querySelector('.flat-group-head')?.textContent)).toEqual([
      '2026-09-20 · 1 night · 10 flats',
      '2026-09-07 – 2026-09-16 · 2 nights · 20 flats',
    ]);
    expect(groups[1]).toHaveTextContent('dust 0.09% · pattern 0.01% against the night before');
    const split = within(families[0]).getByTestId('flat-split');
    expect(split).toHaveTextContent('Split · new dust: 1.6% against a limit of 1.0% · part of a change above');
    // The split sits between the newer group and the older one.
    expect(groups[0].compareDocumentPosition(split) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(split.compareDocumentPosition(groups[1]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    const unmeasured = within(rig).getByRole('list', { name: 'Not measured' });
    expect(unmeasured).toHaveTextContent('2026-09-14 · R · 3° · 2 flats · needs 3 flats');
  });

  it('narrows the families by filter and angle, in the URL', async () => {
    serve();
    renderPage('/calibration?db=demo&calTab=groups&calFilter=G');
    const rig = await screen.findByRole('region', { name: 'UltraCat · ASI6200' });
    await waitFor(() => expect(within(rig).getAllByTestId('flat-family')).toHaveLength(1));
    expect(within(rig).getByRole('heading', { name: 'G · 54°' })).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: 'Angle' }), { target: { value: '3' } });
    await waitFor(() => expect(params().get('calRotation')).toBe('3'));
    expect(within(rig).getByText('No flats of this rig match these filters.')).toBeInTheDocument();
  });

  it('opens a change in the ratio picture, with its spots circled and the stretch in the URL', async () => {
    serve();
    renderPage('/calibration?db=demo&calTab=groups');
    const event = await screen.findByTestId('flat-change-event');
    fireEvent.click(within(event).getByRole('button', { name: 'Compare' }));
    await waitFor(() => expect(params().get('calEarlier')).toBe('r-0916'));
    expect(params().get('calLater')).toBe('r-0920');

    const viewer = await screen.findByRole('region', { name: 'Flat comparison' });
    expect(within(viewer).getByText('2026-09-20 ÷ 2026-09-16', { exact: false })).toBeInTheDocument();
    const image = within(viewer).getByTestId('flat-ratio-image') as HTMLImageElement;
    const src = new URL(image.src);
    expect(src.pathname).toBe('/api/db/demo/calibrations/flat-groups/ratio');
    expect(Object.fromEntries(src.searchParams)).toEqual({ earlier: 'r-0916', later: 'r-0920', stretch: 'normal' });

    const stretch = within(viewer).getByRole('group', { name: 'Stretch' });
    expect(within(stretch).getByRole('button', { name: 'Normal' })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(within(stretch).getByRole('button', { name: 'Tight' }));
    await waitFor(() => expect(params().get('calStretch')).toBe('tight'));
    expect(within(stretch).getByRole('button', { name: 'Tight' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(stretch).getByRole('button', { name: 'Normal' })).toHaveAttribute('aria-pressed', 'false');
    const tight = within(viewer).getByTestId('flat-ratio-image') as HTMLImageElement;
    expect(new URL(tight.src).searchParams.get('stretch')).toBe('tight');

    // Once drawn, the spots are circled on the sensor's own scale.
    fireEvent.load(tight);
    const overlay = await within(viewer).findByRole('img', { name: '2 spots that changed' });
    expect(overlay).toHaveAttribute('viewBox', '0 0 6240 4160');
    const circles = overlay.querySelectorAll('circle');
    expect([...circles].map((circle) => circle.getAttribute('class'))).toEqual([
      'flat-ratio-spot is-new',
      'flat-ratio-spot is-gone',
    ]);

    fireEvent.click(within(viewer).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(params().get('calEarlier')).toBeNull());
    expect(screen.queryByRole('region', { name: 'Flat comparison' })).toBeNull();
  });

  it('opens a split in the ratio picture from its own Compare', async () => {
    serve();
    renderPage('/calibration?db=demo&calTab=groups');
    const split = await screen.findByTestId('flat-split');
    fireEvent.click(within(split).getByRole('button', { name: 'Compare' }));
    const viewer = await screen.findByRole('region', { name: 'Flat comparison' });
    expect(within(viewer).getByRole('heading', { name: 'new dust: 2 shadows near (1418, 3540)' })).toBeInTheDocument();
  });

  it('says why flats group by time, and when none are measured yet', async () => {
    serve({ available: false, rigs: [] });
    const { unmount } = renderPage('/calibration?db=demo&calTab=groups');
    expect(await screen.findByText('This catalog cannot be written, so flats are grouped by time.')).toBeInTheDocument();
    unmount();

    serve({ available: true, rigs: [{ ...RIG, measured: false, events: [], families: [], unmeasured: [] }] });
    const second = renderPage('/calibration?db=demo&calTab=groups');
    expect(await screen.findByText('Grouped by time (Settings › Calibration › Flat groups)')).toBeInTheDocument();
    second.unmount();

    serve({ available: true, rigs: [{ ...RIG, events: [], families: [], unmeasured: [] }] });
    renderPage('/calibration?db=demo&calTab=groups');
    expect(await screen.findByText('No flats measured yet.')).toBeInTheDocument();
  });

  it('shows the flats being measured, and reads the groups again when that ends', async () => {
    const seen = serve();
    let progress: Record<string, unknown> = { running: true, stage: 'measuring flat sessions', stage_done: 3, stage_total: 12 };
    server.use(http.get('/api/db/demo/calibrations/quality', () => ok({
      progress: { to_measure: 0, done: 0, workers: 1, started_at: null, finished_at: null, ...progress },
      counts: {},
    })));
    renderPage('/calibration?db=demo&calTab=groups');
    expect(await screen.findByTestId('flat-groups-measuring')).toHaveTextContent('Measuring flats… 3/12');
    await waitFor(() => expect(seen.groups).toBe(1));
    // The pass ends; the next poll finds it done and the groups are read again.
    progress = { running: false, stage: null, finished_at: 1760000000 };
    await waitFor(() => expect(screen.queryByTestId('flat-groups-measuring')).toBeNull(), { timeout: 6000 });
    await waitFor(() => expect(seen.groups).toBe(2));
  }, 10_000);
});
