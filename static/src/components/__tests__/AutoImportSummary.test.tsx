import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import AutoImportSummary from '../AutoImportSummary';
import { describeAutoImport, relativeTime } from '../../utils/autoimport';
import type { AutoImportSettings, AutoImportStatus, ImportJobProgress } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const ok = <T,>(data: T) => ({ success: true, data, error: null });

const settings: AutoImportSettings = {
  enabled: true,
  on_open: true,
  interval_minutes: 30,
  scope: 'all',
  backfill: true,
  accept_other_rigs: false,
  new_targets: 'ask',
};

const finished: ImportJobProgress = {
  running: false,
  stage: 'complete',
  image_dirs: ['/data/incoming'],
  total_files: 3,
  scanned_files: 3,
  trigger: 'automatic',
  prefiltered: 40,
  outcome: {
    scanned: 43,
    unreadable: 0,
    non_light: 0,
    skipped_processed: 0,
    skipped_out_of_scope: 0,
    skipped_existing: 40,
    skipped_other_rig: 0,
    other_rigs: [],
    imported: 3,
    attached: 3,
    projects_created: 0,
    targets_created: 0,
    templates_created: 0,
    templates_reused: 1,
    plans_created: 0,
    profile_id: 'p',
    dry_run: false,
    project_summaries: [],
    attach_summaries: [],
    created_target_ids: [],
    attached_target_ids: [1],
    calibration: {
      imported: 0,
      updated: 0,
      skipped_existing: 0,
      bias: 0,
      dark: 0,
      dark_flat: 0,
      flat: 0,
    },
  },
};

describe('AutoImportSummary', () => {
  it('renders nothing for a database without automatic import', () => {
    server.use(http.get('/api/db/rig/autoimport', () => HttpResponse.json(ok({}))));
    const { container } = render(<AutoImportSummary dbId="rig" settings={undefined} />, {
      wrapper: wrapper(),
    });
    expect(container).toBeEmptyDOMElement();
  });

  it('describes the schedule and last run, and starts a run now', async () => {
    const now = Math.floor(Date.now() / 1000);
    let runs = 0;
    const status: AutoImportStatus = {
      settings,
      last_started_at: now - 5 * 60,
      next_run_at: now + 25 * 60,
      progress: finished,
    };
    server.use(
      http.get('/api/db/rig/autoimport', () => HttpResponse.json(ok(status))),
      http.post('/api/db/rig/autoimport/run', () => {
        runs += 1;
        return HttpResponse.json(
          ok({ started: true, progress: { ...finished, running: true, stage: 'scanning' } })
        );
      })
    );
    render(<AutoImportSummary dbId="rig" settings={settings} />, { wrapper: wrapper() });
    const line = await screen.findByText(/last run 5 min ago/);
    expect(line.textContent).toContain('Auto import on open and every 30 minutes, lights and calibration');
    expect(line.textContent).toContain('Imported 3 light frame(s)');
    expect(line.textContent).toContain('40 already present');
    expect(line.textContent).toContain('next in 25 min');
    fireEvent.click(screen.getByRole('button', { name: 'Run now' }));
    await waitFor(() => expect(runs).toBe(1));
  });
});

describe('AutoImportSummary notes', () => {
  const held: ImportJobProgress = {
    ...finished,
    outcome: {
      ...finished.outcome!,
      imported: 2,
      attached: 2,
      scan_rules: { files_skipped: 12, folders_skipped: 1, ignored_folders: 0 },
      skipped_by_filter: 3,
      left_out: [{ reason: 'no_target_name', frames: 3, example: '/data/af_01.fits' }],
      new_targets: 'ask',
      skipped_unmatched: 11,
      unmatched: [
        { target: 'NGC 7000', frames: 8, folders: ['/data/NGC7000/LIGHT'] },
        { target: null, frames: 3, folders: ['/data/misc', '/data/NGC7000/LIGHT'] },
      ],
    },
  };

  it('says what the last run left out and offers Review for held frames', async () => {
    const reviewed: string[][] = [];
    server.use(
      http.get('/api/db/rig/autoimport', () =>
        HttpResponse.json(ok({ settings, progress: held, last_started_at: 1 }))
      )
    );
    render(
      <AutoImportSummary
        dbId="rig"
        settings={settings}
        onReview={(folders) => reviewed.push(folders)}
      />,
      { wrapper: wrapper() }
    );
    expect(await screen.findByText('12 skipped by scan rules')).toBeInTheDocument();
    expect(screen.getByText('1 folder(s) skipped by scan rules')).toBeInTheDocument();
    expect(screen.getByText('3 left out: no target name')).toBeInTheDocument();
    expect(screen.getByText('11 held: no matching project')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Review' }));
    expect(reviewed).toEqual([['/data/NGC7000/LIGHT', '/data/misc']]);
  });

  it('offers no Review for frames left out, or without the management gate', async () => {
    server.use(
      http.get('/api/db/rig/autoimport', () =>
        HttpResponse.json(
          ok({
            settings,
            progress: { ...held, outcome: { ...held.outcome!, new_targets: 'skip' } },
            last_started_at: 1,
          })
        )
      ),
      http.get('/api/db/viewer/autoimport', () =>
        HttpResponse.json(ok({ settings, progress: held, last_started_at: 1 }))
      )
    );
    render(<AutoImportSummary dbId="rig" settings={settings} onReview={() => undefined} />, {
      wrapper: wrapper(),
    });
    expect(await screen.findByText('11 left out: no matching project')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Review' })).not.toBeInTheDocument();

    render(<AutoImportSummary dbId="viewer" settings={settings} />, { wrapper: wrapper() });
    expect(await screen.findByText('11 held: no matching project')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Review' })).not.toBeInTheDocument();
  });
});

describe('autoimport words', () => {
  it('phrases schedules and relative times', () => {
    const now = 1_760_000_000_000;
    expect(describeAutoImport({ ...settings, interval_minutes: 0 }, undefined)).toBe(
      'Auto import on open, lights and calibration'
    );
    expect(
      describeAutoImport(
        { ...settings, on_open: false, interval_minutes: 1440, scope: 'calibration' },
        { settings, next_run_at: now / 1000 + 3600 * 3 },
        now
      )
    ).toBe('Auto import every day, calibration frames · next in 3 h');
    expect(relativeTime(now / 1000 - 30, now)).toBe('moments ago');
    expect(relativeTime(now / 1000 - 2 * 86_400, now)).toBe('2 d ago');
  });
});
