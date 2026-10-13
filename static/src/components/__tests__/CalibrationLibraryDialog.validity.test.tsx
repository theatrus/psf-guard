import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import CalibrationLibraryDialog from '../CalibrationLibraryDialog';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

// Two flats one night, one flat two nights later — around a dust cleaning.
// 2026-06-01 20:00 UTC and 2026-06-03 20:00 UTC.
const earlyNight = Math.floor(Date.UTC(2026, 5, 1, 20) / 1000);
const lateNight = Math.floor(Date.UTC(2026, 5, 3, 20) / 1000);

function flat(uuid: string, capturedAt: number, validDirection: string | null = null) {
  return {
    frame_uuid: uuid,
    rig_uuid: 'rig-1',
    kind: 'flat',
    source_path: `/calibration/${uuid}.fits`,
    source_exists: true,
    captured_at: capturedAt,
    camera: 'Camera',
    exposure_s: 3,
    valid_direction: validDirection,
  };
}

const summary = {
  schema_version: 4,
  frame_count: 3,
  master_count: 0,
  rigs: [
    { rig_uuid: 'rig-1', name: 'Rig', frame_count: 3, bias: 0, dark: 0, dark_flat: 0, flat: 3 },
  ],
};

function serveLibrary(frames: unknown[], nightBoundarySeconds?: number) {
  server.use(
    http.get('/api/db/demo/calibrations/details', () =>
      HttpResponse.json({
        success: true,
        data: { summary, frames, night_boundary_seconds: nightBoundarySeconds },
        error: null,
      })
    ),
    http.get('/api/db/demo/calibrations', () =>
      HttpResponse.json({ success: true, data: summary, error: null })
    )
  );
}

describe('calibration validity marking', () => {
  it('groups frames by imaging night and marks a selected night forward', async () => {
    serveLibrary([
      flat('old-a', earlyNight),
      flat('old-b', earlyNight + 600),
      flat('new-a', lateNight),
    ]);
    let received: unknown = null;
    server.use(
      http.put('/api/db/demo/calibrations/frames/validity', async ({ request }) => {
        received = await request.json();
        return HttpResponse.json({ success: true, data: { updated: 2 }, error: null });
      })
    );

    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    // One group per night, newest first.
    expect(await screen.findByText('Night of 2026-06-03')).toBeInTheDocument();
    expect(screen.getByText('Night of 2026-06-01')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('checkbox', { name: 'Select Flats Night of 2026-06-01' }));
    expect(screen.getByText(/2 frames across 1 group/)).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Validity direction'), {
      target: { value: 'backward' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Mark' }));

    await waitFor(() =>
      expect(received).toEqual({
        frame_uuids: ['old-a', 'old-b'],
        direction: 'backward',
      })
    );
  });

  it('dates dawn flats to the night before at a site west of Greenwich', async () => {
    // Lights between 03:00 and 13:00 UTC leave the catalog quiet around
    // 20:00 UTC. Flats at 14:00 UTC on 2 June are 07:00 in California, the
    // dawn after the night of 1 June — N.I.N.A.'s DATEMINUS12 date.
    serveLibrary([flat('dawn', Math.floor(Date.UTC(2026, 5, 2, 14) / 1000))], 20 * 3600);
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    expect(await screen.findByText('Night of 2026-06-01')).toBeInTheDocument();
    expect(screen.queryByText('Night of 2026-06-02')).toBeNull();
  });

  it('shows the validity badge on marked frames', async () => {
    serveLibrary([
      flat('old-a', earlyNight, 'backward'),
      flat('new-a', lateNight, 'forward'),
    ]);
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('◂ before only')).toBeInTheDocument();
    expect(screen.getByText('▸ after only')).toBeInTheDocument();
  });

  it('starts collapsed, separates darks from flats, and forgets a whole night', async () => {
    serveLibrary([
      flat('flat-a', earlyNight),
      { ...flat('dark-a', earlyNight), kind: 'dark' },
      { ...flat('bias-a', earlyNight), kind: 'bias' },
    ]);
    let received: unknown = null;
    server.use(
      http.delete('/api/db/demo/calibrations/frames', async ({ request }) => {
        received = await request.json();
        return HttpResponse.json({
          success: true,
          data: { frames_removed: 1, masters_removed: 0 },
          error: null,
        });
      })
    );

    const { container } = render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    // Darks and bias sit in their own sections beside the flat nights: the
    // same night appears once per section, not merged.
    expect(await screen.findByText('Flats')).toBeInTheDocument();
    const sectionRows = container.querySelectorAll('.calibration-section-row');
    expect([...sectionRows].map((row) => row.querySelector('strong')?.textContent)).toEqual([
      'Flats',
      'Darks',
      'Bias',
    ]);
    expect(screen.getAllByText('Night of 2026-06-01')).toHaveLength(3);

    // Collapsed first: no frame rows until a group is expanded.
    expect(screen.queryByText('flat-a.fits')).toBeNull();
    expect(screen.queryByText('dark-a.fits')).toBeNull();
    const toggles = screen.getAllByRole('button', { name: /Night of 2026-06-01/ });
    fireEvent.click(toggles[0]);
    expect(await screen.findByText('flat-a.fits')).toBeInTheDocument();
    expect(screen.queryByText('dark-a.fits')).toBeNull();

    // Forget night is destructive: it only appears once a night's checkbox
    // is selected, and names only that section's frames.
    expect(screen.queryByRole('button', { name: 'Forget night' })).toBeNull();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select Darks Night of 2026-06-01' }));
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    fireEvent.click(screen.getByRole('button', { name: 'Forget night' }));
    await waitFor(() => expect(received).toEqual({ frame_uuids: ['dark-a'] }));
    confirm.mockRestore();
  });

  it('offers no selection to a read-only viewer', async () => {
    serveLibrary([flat('old-a', earlyNight)]);
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage={false} onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('Night of 2026-06-01')).toBeInTheDocument();
    expect(screen.queryByRole('checkbox', { name: /Select Flats/ })).toBeNull();
  });

  it('marks darks that caught stray light on the night and on the frame', async () => {
    // Not graded yet: the stray-light mark an older check left still shows.
    const why = 'stray light: level 1172.0 sits 669.0 above the quietest dark';
    serveLibrary([
      {
        ...flat('dark-quiet', earlyNight),
        kind: 'dark',
        dark_level: 503,
      },
      {
        ...flat('dark-dawn', earlyNight + 600),
        kind: 'dark',
        dark_level: 1172,
        stray_light: why,
        kept_out: why,
      },
    ]);
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    // The collapsed night already says it holds a frame kept out, which now
    // counts every cause, stray light among them.
    expect(await screen.findByText('1 kept out')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Night of 2026-06-01/ }));
    const badge = await screen.findByText('Stray light');
    expect(badge.getAttribute('title')).toContain('669.0 above the quietest dark');
    expect(screen.getAllByText('Stray light')).toHaveLength(1);
  });

  it('shows the level of each measured dark and nothing for the rest', async () => {
    serveLibrary([
      {
        ...flat('dark-measured', earlyNight),
        kind: 'dark',
        dark_level: 503.4,
        dark_level_sigma: 4.42,
      },
      { ...flat('dark-new', earlyNight + 600), kind: 'dark' },
      {
        ...flat('dark-flat-measured', earlyNight + 300),
        kind: 'dark_flat',
        dark_level: 1172,
        dark_level_sigma: 12.6,
      },
      flat('flat-a', earlyNight + 900),
    ]);
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    // Flats and dark-flats share one night row, darks have another.
    for (const toggle of await screen.findAllByRole('button', { name: /Night of 2026-06-01/ })) {
      fireEvent.click(toggle);
    }
    expect(await screen.findByText('Level 503 ± 4.4 ADU')).toBeInTheDocument();
    expect(screen.getByText('Level 1172 ± 13 ADU')).toBeInTheDocument();
    // The dark not measured yet and the flat carry no level.
    expect(screen.getByText('dark-new.fits')).toBeInTheDocument();
    expect(screen.getByText('flat-a.fits')).toBeInTheDocument();
    expect(screen.getAllByText(/^Level /)).toHaveLength(2);
  });
});

describe('calibration frame grades', () => {
  const graded = [
    { ...flat('ok-flat', earlyNight), grade: 'ok' },
    {
      ...flat('dim-flat', earlyNight + 60),
      grade: 'suspect',
      grade_code: 'dim',
      grade_reason: 'dimmest channel at 7.5% of full scale',
    },
    {
      ...flat('spotted-flat', earlyNight + 120),
      grade: 'poor',
      grade_code: 'changing_features',
      grade_reason: '36 spots that change across its session',
      kept_out: '36 spots that change across its session',
    },
    {
      ...flat('kept-flat', earlyNight + 180),
      grade: 'ok',
      quality_override: 'exclude',
      kept_out: 'by hand',
    },
  ];

  it('shows each grade with its main reason, and the night counts', async () => {
    serveLibrary(graded);
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage={false} onClose={() => {}} />,
      { wrapper: wrapper() }
    );

    expect(await screen.findByText('2 kept out')).toBeInTheDocument();
    expect(screen.getByText('1 suspect')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Night of 2026-06-01/ }));
    expect(await screen.findByText('Suspect · too dim')).toHaveAttribute(
      'title',
      'dimmest channel at 7.5% of full scale'
    );
    expect(screen.getByText('Poor · changing spots')).toHaveAttribute(
      'title',
      '36 spots that change across its session'
    );
    expect(screen.getAllByText('OK')).toHaveLength(2);
    expect(screen.getByText('Kept out')).toBeInTheDocument();
    // A read-only viewer sees the grades but cannot change them.
    expect(screen.queryByRole('button', { name: 'Keep out' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Use anyway' })).toBeNull();
  });

  it('keeps out a good frame, uses a poor one, and clears a choice', async () => {
    serveLibrary(graded);
    const received: unknown[] = [];
    server.use(
      http.put('/api/db/demo/calibrations/frames/quality', async ({ request }) => {
        received.push(await request.json());
        return HttpResponse.json({ success: true, data: { updated: 1 }, error: null });
      })
    );
    render(
      <CalibrationLibraryDialog dbId="demo" dbName="Demo" canManage onClose={() => {}} />,
      { wrapper: wrapper() }
    );
    fireEvent.click(await screen.findByRole('button', { name: /Night of 2026-06-01/ }));
    await screen.findByText('Poor · changing spots');

    // Serving frames offer Keep out; the poor one offers Use anyway; the
    // frame kept out by hand offers Clear.
    const keepOut = screen.getAllByRole('button', { name: 'Keep out' });
    expect(keepOut).toHaveLength(2);
    fireEvent.click(keepOut[0]);
    fireEvent.click(screen.getByRole('button', { name: 'Use anyway' }));
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));

    await waitFor(() => expect(received).toHaveLength(3));
    expect(received).toEqual(
      expect.arrayContaining([
        { frame_uuids: ['spotted-flat'], choice: 'use' },
        { frame_uuids: ['kept-flat'], choice: 'clear' },
      ])
    );
    expect(
      received.filter(
        (body) => (body as { choice: string }).choice === 'exclude'
      )
    ).toHaveLength(1);
  });
});
