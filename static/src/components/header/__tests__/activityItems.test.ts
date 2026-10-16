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
    expect(items[1]).toMatchObject({ status: 'queued', percent: null, detail: 'Waiting for the build ahead' });
  });

  it('sums up as the mean of the running jobs that report progress', () => {
    const items = activityItems(
      [{ dbId: 'a', dbName: 'Askar', scan: scan(4, 10) }],
      [stack('R', 'running', 9, 10), stack('B', 'queued')]
    );
    expect(summarize(items)).toEqual({ count: 3, running: 2, waiting: 1, percent: 65 });
    expect(summarize([])).toEqual({ count: 0, running: 0, waiting: 0, percent: null });
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
      status: 'running',
      percent: null,
      control: { kind: 'wbpp-running', dbId: 'a' },
    });
    expect(items[2]).toMatchObject({
      status: 'queued',
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

  it('lists processing runs in the stack line with what they do and where they stand', () => {
    const build = stack('Ha', 'running', 3, 10);
    const blur: StackActivityEntry = {
      kind: 'processing', job_id: 'b'.repeat(64), database_id: 'db-a', project_id: 1, target_id: 42,
      state: 'running', label: 'M44 · G', detail: 'BlurXTerminator', processed_units: 420,
      total_units: 1000, created_unix_seconds: 2, progress_label: 'BlurXTerminator · 42%',
      processing: {
        operation: 'BlurXTerminator', channel: 'G', stage: 'BlurXTerminator', fraction: 0.42,
        started_unix_seconds: 1_760_000_000, stoppable: true,
      },
    };
    const startedAt = new Date(1_760_000_000_000).toLocaleTimeString([], {
      hour: '2-digit', minute: '2-digit',
    });
    const stretch: StackActivityEntry = {
      kind: 'processing', job_id: 's'.repeat(64), database_id: 'db-a', project_id: 1, target_id: 42,
      state: 'queued', label: 'M44 · R', detail: 'Waiting for BlurXTerminator on M44 · G to finish',
      processed_units: 0, total_units: 0, created_unix_seconds: 3, queue_position: 0,
      processing: {
        operation: 'Deconvolution', channel: 'R', waiting_for: 'BlurXTerminator on M44 · G',
        stoppable: false,
      },
    };
    const items = activityItems([], [stretch, blur, { ...build, state: 'queued', queue_position: 1 }]);
    expect(items.map((item) => item.title)).toEqual(['BlurXTerminator', 'Deconvolution', 'Stacking']);
    expect(items[0]).toMatchObject({
      kind: 'processing',
      scope: 'M44 · G',
      // The title names the tool; the line under it says when it started.
      detail: `started ${startedAt}`,
      status: 'running',
      percent: 42,
      stoppable: true,
      queue: 'stack',
      control: { kind: 'stack', jobId: 'b'.repeat(64) },
      href: '/stacks?db=db-a&project=1&target=42',
    });
    // Waiting: it says what for, has a place in the line, and leaves it like a build.
    expect(items[1]).toMatchObject({
      scope: 'M44 · R',
      detail: 'Waiting for BlurXTerminator on M44 · G',
      status: 'queued',
      percent: null,
      position: 0,
      stoppable: true,
    });
    expect(lineLengths(items)).toEqual({ stack: 2, wbpp: 0 });
    // A running stretch or deconvolution cannot be stopped.
    const deconvolving = activityItems([], [{
      ...stretch, state: 'running', queue_position: null, detail: 'Deconvolution',
      processing: { ...stretch.processing!, waiting_for: null, stage: 'Reading the stack' },
    }]);
    expect(deconvolving[0]).toMatchObject({
      status: 'running', percent: null, stoppable: false, detail: 'Reading the stack',
    });
    expect(summarize(items)).toEqual({ count: 3, running: 1, waiting: 2, percent: 42 });
    // A re-apply the server queued after its stack refreshed is marked.
    expect(activityItems([], [{ ...blur, automatic: true }])[0]).toMatchObject({ automatic: true });
    expect(items[0].automatic).toBeFalsy();
  });

  it('lists automatic work that stepped aside as waiting, after running work', () => {
    const person = stack('P', 'running', 3, 10);
    const automaticBuild = stack('A', 'running', 5, 10, true);
    const items = activityItems(
      [{
        dbId: 'db-a',
        dbName: 'Askar',
        backfill: backfill(2, 8),
        scan: { ...scan(4, 10), progress: { ...scan(4, 10).progress, automatic: true, waiting_for: 'a stack build' } },
      }],
      [automaticBuild, person],
      undefined,
      () => undefined,
      {
        scheduled: [
          {
            database_id: 'db-a', database_name: 'Askar', project_id: 3, project_name: 'Bravo',
            reason: 'arrival', due_in_seconds: 0, channels: ['M44 · Ha'], waiting: true,
            waiting_for: 'a stack build',
            last_progress: { label: 'M44 · Ha', processed_units: 12, total_units: 40 },
          },
          // Still settling: not work yet.
          {
            database_id: 'db-a', database_name: 'Askar', project_id: 4, project_name: 'Charlie',
            reason: 'grade', due_in_seconds: 600, channels: ['M44 · R'], waiting: false,
          },
        ],
        reapplies: [{ database_id: 'db-a', project_id: 3, target_id: 9, label: 'M44 · G', waiting_for: null }],
      }
    );
    // A person's running work, then running background work, then waiting.
    expect(items.map((item) => [item.title, item.status])).toEqual([
      ['Stacking', 'running'],
      ['Stacking', 'running'],
      ['Analyzing database quality', 'waiting'],
      ['Refreshing stacks', 'waiting'],
      ['Re-applying processing', 'waiting'],
    ]);
    expect(items[0].scope).toBe('M44 · P');
    expect(items[1]).toMatchObject({ scope: 'M44 · A', automatic: true, background: true });
    expect(items[2]).toMatchObject({
      detail: 'Waiting for a stack build · 2/8 targets · Solving 4/10 frames',
      waitingFor: 'a stack build',
      // How far it had got stays.
      percent: 25,
    });
    expect(items[3]).toMatchObject({
      key: 'refresh-waiting:db-a:3',
      scope: 'Askar · Bravo',
      detail: 'Waiting for a stack build · stopped at 12/40 frames',
      percent: 30,
      automatic: true,
      href: '/stacks?db=db-a&project=3',
      hint: 'Restacks M44 · Ha',
    });
    // Waiting work has no control: it resumes on its own.
    expect(items[3].control).toBeUndefined();
    expect(items[4]).toMatchObject({
      scope: 'M44 · G',
      detail: 'Waiting to start',
      href: '/stacks?db=db-a&project=3&target=9',
    });

    const summary = summarize(items);
    // Waiting work's last progress does not count toward the chip's.
    expect(summary).toEqual({ count: 5, running: 2, waiting: 3, percent: 40 });
    expect(lineLengths(items)).toEqual({ stack: 0, wbpp: 0 });
  });

  it("lists a person's scan apart from the database analysis waiting for it", () => {
    const items = activityItems(
      [{
        dbId: 'db-a',
        dbName: 'Askar',
        backfill: backfill(2, 8),
        scan: { ...scan(4, 10), progress: { ...scan(4, 10).progress, automatic: false } },
      }],
      []
    );
    expect(items).toMatchObject([
      { key: 'scan:db-a', title: 'Analyzing quality', status: 'running', detail: 'Solving 4/10 frames' },
      {
        key: 'backfill:db-a',
        title: 'Analyzing database quality',
        status: 'waiting',
        waitingFor: 'a quality scan',
        detail: 'Waiting for a quality scan · 2/8 targets',
      },
    ]);
  });
});
