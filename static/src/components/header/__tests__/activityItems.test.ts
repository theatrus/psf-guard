import { describe, expect, it } from 'vitest';
import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  SpatialScanStatus,
  StackActivityEntry,
} from '../../../api/types';
import { activityItems, lineLengths, summarize } from '../activityItems';

function scan(processed: number, total: number, stage = 'astrometry'): SpatialScanStatus {
  return {
    started: false,
    cached_count: 0,
    progress: {
      running: true, stage, target_id: 42, filter_name: null, total, processed,
      skipped_cached: 0, spatial_processed: total, astrometry_processed: processed, solved: processed,
      solve_failed: 0, operational_errors: 0, errors: 0, current_file: 'sh2-86-r-004.fits',
      started_at: 1, finished_at: null, last_error: null,
    },
  };
}

function backfill(processed: number, total: number): QualityBackfillStatus {
  return {
    started: false,
    progress: {
      running: true, force: false, total_targets: total, processed_targets: processed,
      current_target_id: 7, started_at: 1, finished_at: null,
    },
  } as QualityBackfillStatus;
}

function refresh(overrides: Partial<CacheRefreshProgress>): CacheRefreshProgress {
  return {
    is_refreshing: true, stage: 'processing_projects', progress_percentage: 0, elapsed_seconds: 3,
    directories_total: 0, directories_processed: 0, current_directory_name: null, files_scanned: 0,
    projects_total: 0, projects_processed: 0, current_project_name: null, targets_total: 0,
    targets_processed: 0, files_found: 0, files_missing: 0, ...overrides,
  };
}

function stack(
  jobId: string,
  state: 'running' | 'queued',
  processed = 0,
  total = 0,
  automatic = false,
  queuePosition: number | null = null
): StackActivityEntry {
  return {
    kind: 'mono', job_id: jobId, database_id: 'db-a', project_id: 1, state,
    label: `M44 · ${jobId}`, detail: 'registering', processed_units: processed,
    total_units: total, created_unix_seconds: 1, automatic, queue_position: queuePosition,
  };
}

describe('header activity items', () => {
  it('links each job to the view showing what it works on', () => {
    const projectOf = (dbId: string, targetId: number) =>
      dbId === 'db-a' && targetId === 42 ? 4 : dbId === 'db-a' && targetId === 9 ? 2 : undefined;
    const db = { dbId: 'db-a', dbName: 'Askar' };
    const scanning = activityItems(
      [{ ...db, scan: { ...scan(3, 10), progress: { ...scan(3, 10).progress, filter_name: 'Ha' } } }],
      [{ ...stack('R', 'running', 1, 3), target_id: 7 }],
      {
        running: [],
        queued: [{ id: 'q1', db_id: 'db-a', db_name: 'Askar', scope: 'target IC 447', project_id: null, target_id: 9, position: 1, queued_at: 2 }],
      },
      projectOf
    );
    expect(scanning.map((item) => item.href)).toEqual([
      '/sequence?db=db-a&project=4&target=42&filterName=Ha',
      '/stacks?db=db-a&project=1&target=7',
      '/stacks?db=db-a&project=2&target=9',
    ]);

    // A catalog refresh works on the whole database; a backfill between
    // targets on one whose project is not loaded yet shows the database too.
    const idle = activityItems(
      [{ ...db, refresh: refresh({}), backfill: backfill(1, 3) }],
      [],
      undefined,
      projectOf
    );
    expect(idle.map((item) => item.href)).toEqual(['/grid?db=db-a', '/grid?db=db-a']);
  });

  it('describes a quality scan by its frames, and folds it into a running backfill', () => {
    const alone = activityItems([{ dbId: 'a', dbName: 'Askar', scan: scan(4, 10) }], []);
    expect(alone).toMatchObject([
      { title: 'Analyzing quality', scope: 'Askar', detail: 'Solving 4/10 frames', percent: 40 },
    ]);
    const folded = activityItems(
      [{ dbId: 'a', dbName: 'Askar', scan: scan(4, 10, 'spatial'), backfill: backfill(2, 8) }],
      []
    );
    expect(folded).toHaveLength(1);
    expect(folded[0]).toMatchObject({
      title: 'Analyzing database quality',
      detail: '2/8 targets · Scanning 4/10 frames',
      percent: 25,
    });
  });

  it('says where a catalog refresh is, by stage', () => {
    const folders = activityItems([{
      dbId: 'a', dbName: 'Askar',
      refresh: refresh({
        stage: 'initializing_directory_tree', directories_processed: 12, files_scanned: 340,
        current_directory_name: '/mnt/frames/2026-09-30/LIGHT', files_missing: 2,
      }),
    }], []);
    expect(folders[0]).toMatchObject({
      title: 'Refreshing catalog',
      detail: 'Scanning folders · 12 folders, 340 files · LIGHT · 2 missing',
      percent: null,
      hint: '/mnt/frames/2026-09-30/LIGHT',
    });
    const projects = activityItems([{
      dbId: 'a', dbName: 'Askar',
      refresh: refresh({ projects_total: 4, projects_processed: 1, progress_percentage: 25 }),
    }], []);
    expect(projects[0]).toMatchObject({ detail: '1/4 projects', percent: 25 });
    expect(activityItems([{ dbId: 'a', dbName: 'Askar', refresh: refresh({ is_refreshing: false }) }], []))
      .toEqual([]);
  });

  it('lists running builds before queued ones, and a queued build has no progress', () => {
    const items = activityItems([], [
      stack('B', 'queued'),
      stack('R', 'running', 3, 12, true),
      stack('G', 'queued'),
    ]);
    expect(items.map((item) => item.scope)).toEqual(['M44 · R', 'M44 · B', 'M44 · G']);
    expect(items[0]).toMatchObject({ detail: '3/12 frames', percent: 25, automatic: true });
    expect(items[1]).toMatchObject({ queued: true, percent: null, detail: 'Waiting for the build ahead' });
  });

  it('sums up as the mean of the running jobs that report progress', () => {
    const items = activityItems(
      [{ dbId: 'a', dbName: 'Askar', scan: scan(4, 10) }],
      [stack('R', 'running', 9, 10), stack('B', 'queued')]
    );
    expect(summarize(items)).toEqual({ count: 3, queued: 1, percent: 65 });
    expect(summarize([])).toEqual({ count: 0, queued: 0, percent: null });
  });

  it('orders the stack line as the server does and carries each place', () => {
    const items = activityItems([], [
      stack('late', 'queued', 0, 0, false, 1),
      stack('run', 'running', 1, 3),
      stack('next', 'queued', 0, 0, false, 0),
    ]);
    expect(items.map((item) => item.scope)).toEqual(['M44 · run', 'M44 · next', 'M44 · late']);
    expect(items.map((item) => item.position)).toEqual([undefined, 0, 1]);
    expect(items[1].control).toEqual({ kind: 'stack', jobId: 'next' });
  });

  it('lists WBPP runs under way and waiting, each with what it can do', () => {
    const items = activityItems([], [], {
      running: [{
        db_id: 'a', db_name: 'Askar', scope: 'project Bubble', stage: 'running',
        wbpp_stage: 'Image Integration', wbpp_steps: 7, started_at: 1,
      }],
      queued: [
        { id: 'q3', db_id: 'b', db_name: 'Redcat', scope: 'target IC 447', project_id: 2, target_id: 9, position: 1, queued_at: 2 },
        { id: 'q4', db_id: 'a', db_name: 'Askar', scope: 'project Heart', project_id: 3, target_id: null, position: 2, queued_at: 3 },
      ],
    });
    expect(items[0]).toMatchObject({
      title: 'WBPP',
      scope: 'Askar · project Bubble',
      detail: 'Running in PixInsight · Image Integration · 7 steps done',
      queued: false,
      percent: null,
      control: { kind: 'wbpp-running', dbId: 'a' },
    });
    expect(items[2]).toMatchObject({
      queued: true,
      position: 1,
      control: { kind: 'wbpp-queued', dbId: 'a', queueId: 'q4' },
    });
    expect(lineLengths(items)).toEqual({ stack: 0, wbpp: 2 });
  });

  it('shows the final pass in words', () => {
    const running = { ...stack('R', 'running', 21, 34), progress_label: 'Rejecting transients · pass 2/3 · frame 3/8' };
    const color = { ...stack('C', 'running', 1, 4), kind: 'color' as const };
    const items = activityItems([], [running, color]);
    expect(items[0]).toMatchObject({
      detail: 'Rejecting transients · pass 2/3 · frame 3/8',
      percent: (21 / 34) * 100,
      stoppable: true,
    });
    // A running color composition finishes once it starts.
    expect(items[1]).toMatchObject({ title: 'Composing color', stoppable: false });
    expect(items).toHaveLength(2);
  });
});
