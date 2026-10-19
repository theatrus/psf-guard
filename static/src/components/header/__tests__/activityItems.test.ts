import { describe, expect, it } from 'vitest';
import type {
  CacheRefreshProgress,
  CatalogInstallProgress,
  CalibrationDuplicateStatus,
  RejectRemovalStatus,
  FlatMeasureStatus,
  FrameSitesStatus,
  QualityBackfillStatus,
  SpatialScanStatus,
  StackActivityEntry,
} from '../../../api/types';
import {
  activityItems,
  flatRigsLeft,
  frameSitesLabel,
  groupActivity,
  lineLengths,
  queueText,
  summarize,
  tierOf,
  transferItem,
} from '../activityItems';
import type { BackupJob } from '../../../api/backupTypes';
import type { TransferJob } from '../../../api/transferTypes';

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
  state: 'running' | 'queued' | 'preparing',
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
  it('shows a build being prepared as running, stoppable, with what it reads', () => {
    const [item] = activityItems(
      [],
      [{ ...stack('asked', 'preparing'), label: '1263 frames', detail: 'Reading frames and matching calibration' }],
      undefined,
      () => undefined,
      {},
      []
    );
    expect(item).toMatchObject({
      title: 'Preparing stack build',
      scope: '1263 frames',
      detail: 'Reading frames and matching calibration',
      status: 'running',
      percent: null,
      stoppable: true,
      position: undefined,
    });
  });

  it('shows a color composition being prepared as waiting for its channel builds', () => {
    const [item] = activityItems(
      [],
      [{
        ...stack('composition', 'preparing'), kind: 'color', label: 'RGB',
        detail: 'Waiting for the channel builds to finish',
      }],
      undefined,
      () => undefined,
      {},
      []
    );
    expect(item).toMatchObject({
      title: 'Preparing color preview',
      scope: 'RGB',
      detail: 'Waiting for the channel builds to finish',
      stoppable: true,
    });
  });

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

  it('says a composition in the quick lane waits for a composition, not a build', () => {
    const color = (queuePosition: number | null): StackActivityEntry => ({
      ...stack('RGB', 'queued', 0, 0, false, queuePosition), kind: 'color',
    });
    expect(activityItems([], [color(null)])[0]).toMatchObject({ detail: 'Waiting for the composition ahead' });
    expect(activityItems([], [color(0)])[0]).toMatchObject({ detail: 'Waiting for the build ahead' });
  });

  it('lists running builds before queued ones, and a queued build has no progress', () => {
    const items = activityItems([], [
      stack('B', 'queued'),
      stack('R', 'running', 3, 12),
      stack('G', 'queued'),
    ]);
    expect(items.map((item) => item.scope)).toEqual(['M44 · R', 'M44 · B', 'M44 · G']);
    expect(items[0]).toMatchObject({ detail: '3/12 frames', percent: 25 });
    expect(items[1]).toMatchObject({ status: 'queued', percent: null, detail: 'Waiting for the build ahead' });
  });

  it("lists a person's work before automatic work, even an automatic build running", () => {
    const items = activityItems([], [
      stack('B', 'queued'),
      stack('R', 'running', 3, 12, true),
      stack('G', 'queued'),
    ]);
    expect(items.map((item) => item.scope)).toEqual(['M44 · B', 'M44 · G', 'M44 · R']);
    expect(items.map(tierOf)).toEqual(['person', 'person', 'automatic']);
    expect(items[2]).toMatchObject({ status: 'running', automatic: true });
  });

  it('sums up as the mean of the running jobs that report progress', () => {
    const items = activityItems(
      [{ dbId: 'a', dbName: 'Askar', scan: scan(4, 10) }],
      [stack('R', 'running', 9, 10), stack('B', 'queued')]
    );
    expect(summarize(items)).toEqual({ count: 3, attention: 0, running: 2, waiting: 0, queued: 1, percent: 65 });
    expect(summarize([])).toEqual({ count: 0, attention: 0, running: 0, waiting: 0, queued: 0, percent: null });
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
    // The server counts a color job in hundredths of a step, so an RC-Astro
    // tool's own progress moves the percent.
    const color = {
      ...stack('C', 'running', 1250, 4000),
      kind: 'color' as const,
      progress_label: 'H-alpha: BlurXTerminator (42%)',
    };
    const items = activityItems([], [running, color]);
    expect(items[0]).toMatchObject({
      detail: 'Rejecting transients · pass 2/3 · frame 3/8',
      percent: (21 / 34) * 100,
      stoppable: true,
    });
    // A running color composition says what it is doing, and stops at its
    // next channel or step.
    expect(items[1]).toMatchObject({
      title: 'Composing color',
      detail: 'H-alpha: BlurXTerminator (42%)',
      percent: 31.25,
      stoppable: true,
    });
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
    expect(summarize(items)).toEqual({ count: 3, attention: 0, running: 1, waiting: 0, queued: 2, percent: 42 });
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
    expect(summary).toEqual({ count: 5, attention: 0, running: 2, waiting: 3, queued: 0, percent: 40 });
    expect(lineLengths(items)).toEqual({ stack: 0, wbpp: 0 });
  });

  it("lists a re-apply that follows a person's build as their work", () => {
    const items = activityItems([], [], undefined, undefined, {
      reapplies: [
        { database_id: 'db-a', project_id: 3, target_id: 9, label: 'M44 · G', personal: true,
          waiting_for: 'the processing of M44 · R' },
        { database_id: 'db-a', project_id: 3, target_id: 9, label: 'M44 · B', waiting_for: 'a stack build' },
      ],
    });
    expect(items).toMatchObject([
      {
        scope: 'M44 · G',
        status: 'waiting',
        detail: 'Waiting for the processing of M44 · R',
        automatic: false,
        background: false,
      },
      { scope: 'M44 · B', automatic: true, background: true },
    ]);
    expect(items.map(tierOf)).toEqual(['person', 'automatic']);
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

describe('flat measuring in the job line', () => {
  const idle: FlatMeasureStatus = {
    state: 'idle', active: false, rig_uuid: null, rig_name: null, stage: null, done: 0, total: 0, rigs: [],
    stopped: [], retry_at: null, priority: null, waiting_for: null, workers: 0, started_at: null, finished_at: null,
    last: null, last_error: null,
  };
  const measuring: FlatMeasureStatus = {
    ...idle, state: 'running', active: true, rig_uuid: 'c925', rig_name: 'C925 · ASI2600', priority: 'background',
    stage: 'measuring flat sessions', done: 12, total: 40, workers: 2,
  };
  const db = { dbId: 'astro', dbName: 'Observatory' };

  it('shows a pass with work as an automatic job, with its rig, progress, a stop and a link', () => {
    const [item] = activityItems([{ ...db, flats: measuring }], []);
    expect(item).toMatchObject({
      key: 'flats:astro',
      kind: 'flats',
      title: 'Measuring flats',
      scope: 'Observatory · C925 · ASI2600',
      detail: 'Measuring flats · 12 of 40 sessions',
      status: 'running',
      percent: 30,
      automatic: true,
      background: true,
      stoppable: true,
      control: { kind: 'flats', dbId: 'astro' },
      href: '/calibration?db=astro&calKind=flat&calRig=c925',
    });
    expect(summarize([item])).toEqual({ count: 1, attention: 0, running: 1, waiting: 0, queued: 0, percent: 30 });
  });

  it('leaves out a pass that is settling, waiting for grading or has nothing to read', () => {
    expect(activityItems([{ ...db, flats: idle }], [])).toEqual([]);
    expect(activityItems([{ ...db, flats: { ...idle, state: 'queued', rigs: ['c925'] } }], [])).toEqual([]);
    expect(activityItems([{ ...db, flats: { ...idle, state: 'stopped', stopped: ['c925'] } }], [])).toEqual([]);
    expect(activityItems([{ ...db, flats: { ...measuring, active: false } }], [])).toEqual([]);
  });

  it('stays automatic when a page puts it first, and is listed as waiting while it steps aside', () => {
    const [viewed] = activityItems([{ ...db, flats: { ...measuring, priority: 'viewed' } }], []);
    expect(viewed.automatic).toBe(true);
    const [waiting] = activityItems([{
      ...db, flats: { ...measuring, stage: 'comparing flat sessions', done: 1, total: 4, waiting_for: 'a stack build' },
    }], []);
    expect(waiting).toMatchObject({
      status: 'waiting',
      waitingFor: 'a stack build',
      detail: 'Waiting for a stack build · Comparing flats · 1 of 4 pairs',
      percent: 25,
    });
    expect(summarize([waiting])).toMatchObject({ running: 0, waiting: 1 });
  });
});

describe('reading frame sites in the job line', () => {
  const db = { dbId: 'astro', dbName: 'Observatory' };
  const reading: FrameSitesStatus = {
    state: 'running', active: true, done: 812, total: 1867, waiting_for: null, retry_at: null,
    started_at: 1, finished_at: null, last: null, last_error: null,
  };

  it('shows a pass with frames to read as an automatic job, with its progress and a stop', () => {
    const [item] = activityItems([{ ...db, sites: reading }], []);
    expect(item).toMatchObject({
      key: 'sites:astro',
      kind: 'sites',
      title: 'Reading frame sites',
      scope: 'Observatory',
      detail: 'Reading frame sites · 812 of 1,867 frames',
      status: 'running',
      automatic: true,
      background: true,
      stoppable: true,
      control: { kind: 'sites', dbId: 'astro' },
    });
    expect(item.percent).toBeCloseTo(43.49, 1);
  });

  it('is listed as waiting while it steps aside, and left out at rest or retrying', () => {
    const [waiting] = activityItems([{ ...db, sites: { ...reading, waiting_for: 'a stack build' } }], []);
    expect(waiting).toMatchObject({
      status: 'waiting',
      detail: 'Waiting for a stack build · Reading frame sites · 812 of 1,867 frames',
    });
    expect(activityItems([{ ...db, sites: { ...reading, state: 'queued', active: false, retry_at: 99 } }], []))
      .toEqual([]);
    expect(frameSitesLabel({ done: 0, total: 1 })).toBe('Reading frame sites · 0 of 1 frame');
  });
});

describe('reject removal in the job line', () => {
  const idle: RejectRemovalStatus = {
    state: 'idle', action: null, stage: null, done: 0, total: 0, run: 0, started_at: null, finished_at: null,
    plan: null, changed: false, outcome: null, error: null, priority: null,
  };
  const db = { dbId: 'c925', dbName: 'C925' };

  it("lists a person's removal with how far it got and a stop, and nothing once it ends", () => {
    const [item] = activityItems([{
      ...db, rejects: { ...idle, state: 'running', action: 'remove', stage: 'moving', done: 37, total: 203, run: 3, priority: 'interactive' },
    }], []);
    expect(item).toMatchObject({
      key: 'rejects:c925',
      kind: 'rejects',
      title: 'Removing rejects',
      scope: 'C925',
      detail: 'Moving to the trash 37 of 203',
      status: 'running',
      stoppable: true,
      control: { kind: 'rejects', dbId: 'c925' },
    });
    expect(item.background).toBeFalsy();
    expect(item.percent).toBeCloseTo(18.23, 1);
    const [preview] = activityItems([{
      ...db, rejects: { ...idle, state: 'running', action: 'preview', stage: 'indexing', done: 2, total: 5, run: 1 },
    }], []);
    expect(preview).toMatchObject({ title: 'Previewing reject removal', detail: 'Indexing folders 2 of 5' });
    expect(activityItems([{ ...db, rejects: { ...idle, state: 'stopped', action: 'remove', run: 3 } }], [])).toEqual([]);
  });
});

describe('duplicate checks and repairs in the job line', () => {
  const idle: CalibrationDuplicateStatus = {
    state: 'idle', action: null, stage: null, done: 0, total: 0, run: 0, started_at: null, finished_at: null,
    report: null, checked_at: null, repaired: null, changed: null, error: null,
  };
  const db = { dbId: 'ultracat', dbName: 'Ultracat' };

  it("shows a person's check with its files, a stop and a link to the duplicates", () => {
    const [item] = activityItems([{
      ...db, duplicates: { ...idle, state: 'running', action: 'check', stage: 'checking_files', done: 812, total: 2140, run: 1 },
    }], []);
    expect(item).toMatchObject({
      key: 'duplicates:ultracat',
      kind: 'duplicates',
      title: 'Checking for duplicates',
      scope: 'Ultracat',
      detail: 'Checking files 812 of 2,140',
      status: 'running',
      stoppable: true,
      control: { kind: 'duplicates', dbId: 'ultracat' },
      href: '/calibration?db=ultracat&calDupes=true',
    });
    expect(item.automatic).toBeFalsy();
    expect(item.percent).toBeCloseTo(37.94, 1);
  });

  it('says a repair merges, and leaves out a run that has ended', () => {
    const [item] = activityItems([{
      ...db, duplicates: { ...idle, state: 'running', action: 'repair', stage: 'merging', done: 37, total: 203, run: 2 },
    }], []);
    expect(item).toMatchObject({ title: 'Repairing duplicates', detail: 'Merging 37 of 203' });
    // The check a page starts on its own is background work, listed as
    // waiting while it steps aside for a person's.
    const [waiting] = activityItems([{
      ...db, duplicates: {
        ...idle, state: 'running', action: 'check', stage: 'checking_files', run: 3,
        priority: 'background', waiting_for: 'a stack build',
      },
    }], []);
    expect(waiting).toMatchObject({
      status: 'waiting', waitingFor: 'a stack build', automatic: true, background: true,
      detail: 'Waiting for a stack build · Checking files…',
    });
    for (const state of ['idle', 'done', 'stopped', 'failed'] as const) {
      expect(activityItems([{ ...db, duplicates: { ...idle, state } }], [])).toEqual([]);
    }
  });
});

describe('backups in the job line', () => {
  const job: BackupJob = {
    trigger: 'manual', scope: 'full', include_secrets: false, destination: '/backups',
    location: 'elsewhere', stage: 'copying', current: 'catalog imaging-rig', files_done: 2,
    files_total: 8, bytes_done: 30, bytes_total: 120, waiting_for: null, started_at: 1700,
    stopping: false,
  };

  it("shows a person's backup as running work with its progress and a stop", () => {
    const [item] = activityItems([], [], undefined, undefined, {}, [], job);
    expect(item).toMatchObject({
      key: 'backup:1700',
      kind: 'backup',
      title: 'Backing up',
      scope: 'Settings and catalogs',
      detail: 'Copying catalog imaging-rig · 2/8 files',
      status: 'running',
      percent: 25,
      automatic: false,
      background: false,
      stoppable: true,
      control: { kind: 'backup' },
      hint: '/backups',
    });
    expect(item.href).toBeUndefined();
  });

  it('lists a scheduled backup as automatic, and as waiting while it steps aside', () => {
    const [item] = activityItems([], [], undefined, undefined, {}, [], {
      ...job, trigger: 'schedule', scope: 'meta', waiting_for: 'a stack build',
    });
    expect(item).toMatchObject({
      title: 'Scheduled backup',
      scope: 'Settings',
      status: 'waiting',
      waitingFor: 'a stack build',
      detail: 'Waiting for a stack build',
      automatic: true,
      background: true,
    });
    expect(activityItems([], [], undefined, undefined, {}, [], null)).toEqual([]);
  });
});

describe('catalog installs in the job line', () => {
  const progress: CatalogInstallProgress = {
    running: true, phase: 'downloading', message: 'Downloading stars-gaia-photometry.bin…',
    preset: 'gaia_photometry', output_dir: '/catalogs', file_name: 'stars-gaia-photometry.bin',
    files_completed: 0, files_total: 1, bytes_completed: 119, bytes_total: 476, started_at: 1800,
  };

  it("shows a person's Gaia colors download with its progress", () => {
    const [item] = activityItems([], [], undefined, undefined, {}, [], null, progress);
    expect(item).toMatchObject({
      key: 'catalog:1800',
      kind: 'catalog',
      title: 'Installing Gaia colors',
      scope: 'Seiza catalogs',
      detail: 'Downloading stars-gaia-photometry.bin…',
      status: 'running',
      percent: 25,
      hint: '/catalogs',
    });
    expect(tierOf(item)).toBe('person');
    expect(item.href).toBeUndefined();
  });

  it('counts finished files when the download gives no size, and leaves a finished install out', () => {
    const [item] = activityItems([], [], undefined, undefined, {}, [], null, {
      ...progress, preset: 'blind_deep', phase: 'installing', message: 'Installing objects.bin…',
      bytes_completed: undefined, bytes_total: undefined, files_completed: 3, files_total: 6,
    });
    expect(item).toMatchObject({ title: 'Installing catalogs', percent: 50 });
    expect(activityItems([], [], undefined, undefined, {}, [], null, {
      ...progress, running: false, phase: 'complete',
    })).toEqual([]);
  });
});

describe('the job line by tier', () => {
  const wbppQueued = (id: string, position: number) => ({
    id, db_id: 'a', db_name: 'Askar', scope: `project ${id}`, project_id: 1, target_id: null, position,
    queued_at: position,
  });
  const transfer = (overrides: Partial<TransferJob>): TransferJob => ({
    id: 't', connection_id: 'c', revision: 1, state: 'queued', catalog: 'Askar', label: 'M44',
    total_files: 10, completed_files: 4, total_bytes: 100, completed_bytes: 40, current_file: '',
    sent_bytes: 0, error: null, next_attempt_ms: 0, warnings: null, manifest: null, ...overrides,
  });

  it("puts a person's work, then automatic work, each needing attention, running, queued in line order, then waiting", () => {
    const items = activityItems(
      [{
        dbId: 'a',
        dbName: 'Askar',
        refresh: refresh({ stage: 'processing_targets', targets_total: 15, targets_processed: 3, automatic: true }),
        backfill: backfill(2, 8),
        scan: { ...scan(4, 10), progress: { ...scan(4, 10).progress, automatic: true, waiting_for: 'a stack build' } },
      }],
      [
        stack('second', 'queued', 0, 0, false, 1),
        stack('auto', 'queued', 0, 0, true, 2),
        stack('run', 'running', 3, 12),
        stack('first', 'queued', 0, 0, false, 0),
      ],
      // The server lists the WBPP line next first.
      { running: [], queued: [wbppQueued('next', 1), wbppQueued('late', 2)] },
      () => undefined,
      {},
      [
        transfer({ id: 'paused', state: 'paused', label: 'M13' }),
        transfer({ id: 'held', state: 'needs_review' }),
        transfer({ id: 'policy', state: 'queued', automatic: true }),
      ]
    );
    const groups = groupActivity(items);
    expect(groups.map((group) => group.title)).toEqual(['Started by you', 'Automatic']);
    expect(groups[0].items.map((item) => [item.title, item.scope, item.status])).toEqual([
      ['Transfers', 'Askar · M44', 'attention'],
      ['Stacking', 'M44 · run', 'running'],
      ['Stacking', 'M44 · first', 'queued'],
      ['Stacking', 'M44 · second', 'queued'],
      ['WBPP', 'Askar · project next', 'queued'],
      ['WBPP', 'Askar · project late', 'queued'],
      ['Transfers', 'Askar · M13', 'waiting'],
    ]);
    expect(groups[0].summary).toMatchObject({ attention: 1, running: 1, queued: 4, waiting: 1 });
    expect(groups[1].items.map((item) => [item.title, item.status])).toEqual([
      ['Refreshing catalog', 'running'],
      ['Stacking', 'queued'],
      ['Transfers', 'queued'],
      ['Analyzing database quality', 'waiting'],
    ]);
    expect(groups[1].summary).toMatchObject({ attention: 0, running: 1, queued: 2, waiting: 1 });
    // The flat list is the groups in turn, and the chip counts them all.
    expect(items).toEqual([...groups[0].items, ...groups[1].items]);
    expect(summarize(items)).toMatchObject({ attention: 1, running: 2, queued: 6, waiting: 2 });
  });

  it("says a queued build's place in the whole line, under whichever heading it sits", () => {
    // An automatic refresh's build is queued ahead of a person's.
    const items = activityItems([], [
      stack('run', 'running', 3, 12),
      stack('refresh', 'queued', 0, 0, true, 0),
      stack('mine', 'queued', 0, 0, false, 1),
    ]);
    const [person, automatic] = groupActivity(items);
    const mine = person.items.find((item) => item.scope === 'M44 · mine')!;
    const refreshBuild = automatic.items.find((item) => item.scope === 'M44 · refresh')!;
    // The person's build is first under its heading, yet one build is ahead.
    expect(person.items.map((item) => item.status)).toEqual(['running', 'queued']);
    expect(queueText(mine)).toBe('queued · 2 · 1 ahead');
    expect(queueText(refreshBuild)).toBe('queued · 1 · next');
    // The move buttons work on the whole line: up takes the person's build
    // past the refresh, down is the end.
    const lengths = lineLengths(items);
    expect(lengths.stack).toBe(2);
    expect(mine.position! - 1).toBe(0);
    expect(mine.position).toBe(lengths.stack - 1);
    expect(queueText({ ...mine, queue: undefined })).toBe('queued');
  });

  it('lists every queued build and WBPP run, not only the next', () => {
    const queued = Array.from({ length: 5 }, (_, index) => stack(`q${index}`, 'queued', 0, 0, false, index));
    const runs = Array.from({ length: 4 }, (_, index) => wbppQueued(`w${index}`, index + 1));
    const items = activityItems([], queued, { running: [], queued: runs });
    expect(items.filter((item) => item.queue === 'stack').map((item) => item.position)).toEqual([0, 1, 2, 3, 4]);
    expect(items.filter((item) => item.queue === 'wbpp').map((item) => item.position)).toEqual([0, 1, 2, 3]);
    expect(lineLengths(items)).toEqual({ stack: 5, wbpp: 4 });
  });

  it('leaves out a heading with nothing under it', () => {
    const groups = groupActivity(activityItems([], [stack('run', 'running', 1, 2)]));
    expect(groups.map((group) => group.tier)).toEqual(['person']);
    expect(groupActivity([])).toEqual([]);
  });

  it('files a catalog refresh by who asked for it', () => {
    const asked = activityItems([{ dbId: 'a', dbName: 'Askar', refresh: refresh({ automatic: false }) }], []);
    const expired = activityItems([{ dbId: 'a', dbName: 'Askar', refresh: refresh({ automatic: true }) }], []);
    expect(tierOf(asked[0])).toBe('person');
    expect(tierOf(expired[0])).toBe('automatic');
  });

  it('files a transfer by who queued it, and only one held for review needs attention', () => {
    expect(tierOf(transferItem(transfer({ automatic: true })))).toBe('automatic');
    expect(tierOf(transferItem(transfer({})))).toBe('person');
    // Stopped until a person reviews it: before upload, or after it failed.
    expect(transferItem(transfer({ state: 'review', automatic: true })).status).toBe('attention');
    expect(transferItem(transfer({ state: 'needs_review' })).status).toBe('attention');
    // A retry resolves on its own; a paused one waits for whoever paused it.
    expect(transferItem(transfer({ state: 'retry', error: 'Connection reset' })).status).toBe('waiting');
    expect(transferItem(transfer({ state: 'paused' })).status).toBe('waiting');
  });
});

describe('the work left behind each row', () => {
  it('adds nothing the detail already says', () => {
    const rows = activityItems(
      [{
        dbId: 'a',
        dbName: 'Askar',
        refresh: refresh({ stage: 'processing_targets', targets_total: 15, targets_processed: 3 }),
        backfill: backfill(2, 8),
        scan: scan(4, 10),
      }],
      [stack('R', 'running', 12, 40), { ...stack('RGB', 'running', 1, 4), kind: 'color' }],
      undefined,
      () => undefined,
      {},
      [],
      {
        trigger: 'manual', scope: 'meta', include_secrets: false, destination: '/b', location: 'destination',
        stage: 'copying', current: null, files_done: 2, files_total: 6, bytes_done: 1, bytes_total: 4,
        waiting_for: null, started_at: 1, stopping: false,
      }
    );
    expect(rows.length).toBeGreaterThan(4);
    for (const row of rows) expect(row.remaining, row.title).toBeUndefined();
  });

  it('says how many rigs flat measuring has left to check, and what waits for its next pass', () => {
    expect(flatRigsLeft({ rigs_after: 2, next_pass: [] })).toBe('2 more rigs to check');
    expect(flatRigsLeft({ rigs_after: 1, next_pass: ['x', 'y', 'z'] })).toBe('1 more rig to check · 3 rigs queued');
    expect(flatRigsLeft({ rigs_after: 0, next_pass: ['*'] })).toBe('every rig queued');
    // A server without the fields says nothing.
    expect(flatRigsLeft({})).toBeUndefined();
  });

  it('says when another frame-sites pass waits, and how many channels a waiting refresh stacks', () => {
    const [sites] = activityItems([{
      dbId: 'a', dbName: 'Askar',
      sites: {
        state: 'running', active: true, done: 800, total: 1000, next_pass: true, waiting_for: null,
        retry_at: null, started_at: 1, finished_at: null, last: null, last_error: null,
      },
    }], []);
    expect(sites.remaining).toBe('another pass queued');
    const [refreshRow] = activityItems([], [], undefined, () => undefined, {
      scheduled: [{
        database_id: 'a', database_name: 'Askar', project_id: 3, project_name: 'Bravo', reason: 'arrival',
        due_in_seconds: 0, channels: ['M44 · R', 'M44 · G'], waiting: true, waiting_for: 'a stack build',
      }],
    });
    expect(refreshRow.remaining).toBe('2 channels to stack');
  });
});

it('shows a master copy as active without offering to stop PixInsight', () => {
  const items = activityItems([], [], { running: [{ db_id: 'a', db_name: 'Askar', scope: 'Heart', stage: 'publishing',
    wbpp_stage: '1 of 3 files processed · masterLight.xisf', wbpp_steps: 0, started_at: 1 }], queued: [] });
  expect(items[0]).toMatchObject({ status: 'running', stoppable: false,
    detail: 'Copying masters · 1 of 3 files processed · masterLight.xisf' });
  expect(items[0].control).toBeUndefined();
});
