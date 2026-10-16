import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  ScheduledRefresh,
  SpatialScanStatus,
  StackActivityEntry,
  WaitingReapply,
  WbppActivity,
} from '../../api/types';

/** What a row can do: stop its job, and move it in its line. */
export type ActivityControl =
  | { kind: 'stack'; jobId: string }
  | { kind: 'wbpp-running'; dbId: string }
  | { kind: 'wbpp-queued'; dbId: string; queueId: string };

/** The two lines a person can reorder. Each runs one job at a time. */
export type ActivityQueue = 'stack' | 'wbpp';

/**
 * Where a job stands. `running`: at work now. `queued`: in a line a person
 * can reorder. `waiting`: automatic work that has stepped aside for other
 * work, such as a build a person started; it resumes on its own.
 */
export type ActivityStatus = 'running' | 'queued' | 'waiting';

/** One piece of background work the header reports. */
export interface ActivityItem {
  key: string;
  kind: 'refresh' | 'quality' | 'stack' | 'processing' | 'wbpp';
  /** What is happening: "Stacking", "Analyzing quality". */
  title: string;
  /** What it happens to: a database, or a target and channel. */
  scope: string;
  /** The progress in words: "1/3 frames", a stage, a file. */
  detail: string;
  status: ActivityStatus;
  /** What waiting work waits for, when the server says: "a stack build". */
  waitingFor?: string;
  /** 0–100, or null when the work cannot say how far along it is. Waiting
   *  work keeps how far it had got. */
  percent: number | null;
  automatic?: boolean;
  /** Background work, automatic or database-wide, which runs after
   *  anything a person started. */
  background?: boolean;
  /** The full current path or file, for a tooltip. */
  hint?: string;
  /** The line this job runs in, when it has one. */
  queue?: ActivityQueue;
  /** Place among the waiting jobs of its line, 0 next. */
  position?: number;
  control?: ActivityControl;
  /** Whether Stop can end it while it runs. A color composition cannot. */
  stoppable?: boolean;
  /** The view showing what the job works on, scoped by URL. */
  href?: string;
}

/** The project a target belongs to, from the header's target list, or
 *  undefined while that list has not loaded. */
export type ProjectOf = (dbId: string, targetId: number) => number | undefined;

function viewPath(
  view: 'grid' | 'sequence' | 'stacks',
  params: Record<string, string | number | null | undefined>
): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value != null) search.set(key, String(value));
  }
  return `/${view}?${search.toString()}`;
}

/** A view at one target, or the database's images while its project is not
 *  known. */
function targetPath(
  view: 'sequence' | 'stacks',
  dbId: string,
  targetId: number | null | undefined,
  projectOf: ProjectOf,
  projectId?: number | null,
  extra: Record<string, string | null | undefined> = {}
): string {
  const project = projectId ?? (targetId != null ? projectOf(dbId, targetId) : undefined);
  if (project == null) return viewPath('grid', { db: dbId });
  return viewPath(view, { db: dbId, project, target: targetId, ...extra });
}

export interface DatabaseActivity {
  dbId: string;
  dbName: string;
  refresh?: CacheRefreshProgress;
  scan?: SpatialScanStatus;
  backfill?: QualityBackfillStatus;
}

/** Automatic stack work out of the line, waiting for the worker. */
export interface WaitingStackWork {
  /** Refreshes the server lists; only those due and waiting are shown. */
  scheduled?: ScheduledRefresh[];
  reapplies?: WaitingReapply[];
}

const REFRESH_STAGES: Record<string, string> = {
  initializing_directory_tree: 'Scanning folders',
  loading_projects: 'Loading projects',
  processing_projects: 'Processing projects',
  processing_targets: 'Processing targets',
  updating_cache: 'Updating cache',
  completed: 'Finishing',
};

function fraction(done: number, total: number): number | null {
  return total > 0 ? Math.min((done / total) * 100, 100) : null;
}

function lastPart(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** `Waiting for a stack build · 2/8 targets`: what it waits for, then how
 *  far it had got. */
function waitingDetail(waitingFor: string | null | undefined, progress = ''): string {
  return [waitingFor ? `Waiting for ${waitingFor}` : 'Waiting to start', progress]
    .filter(Boolean)
    .join(' · ');
}

function refreshItem(db: DatabaseActivity, refresh: CacheRefreshProgress): ActivityItem {
  let detail = REFRESH_STAGES[refresh.stage] ?? refresh.stage;
  if (refresh.stage === 'processing_projects' && refresh.projects_total > 0) {
    detail = `${refresh.projects_processed}/${refresh.projects_total} projects`;
  } else if (refresh.stage === 'processing_targets' && refresh.targets_total > 0) {
    detail = `${refresh.targets_processed}/${refresh.targets_total} targets`;
  } else if (refresh.stage === 'initializing_directory_tree') {
    const counts = [
      refresh.directories_processed > 0 ? `${refresh.directories_processed} folders` : '',
      refresh.files_scanned > 0 ? `${refresh.files_scanned} files` : '',
    ].filter(Boolean).join(', ');
    const place = refresh.current_directory_name ? lastPart(refresh.current_directory_name) : '';
    detail = ['Scanning folders', counts, place].filter(Boolean).join(' · ');
  }
  if (refresh.files_missing > 0) detail += ` · ${refresh.files_missing} missing`;
  return {
    key: `refresh:${db.dbId}`,
    kind: 'refresh',
    title: 'Refreshing catalog',
    scope: db.dbName,
    detail,
    status: 'running',
    percent: refresh.progress_percentage > 0 ? Math.min(refresh.progress_percentage, 100) : null,
    hint: refresh.current_directory_name ?? refresh.current_project_name ?? undefined,
    href: viewPath('grid', { db: db.dbId }),
  };
}

/**
 * A database's quality work: a scan, a database-wide analysis, or both. The
 * analysis runs its scans one target at a time, so its row carries the scan
 * under way. A scan a person started takes the database's scanner, and the
 * analysis waits for it in a row of its own.
 */
function qualityItems(db: DatabaseActivity, projectOf: ProjectOf): ActivityItem[] {
  const backfill = db.backfill?.progress;
  const scan = db.scan?.progress;
  const verb = scan?.stage === 'astrometry' ? 'Solving' : 'Scanning';
  // The target the scan is on: its sequence shows the scores it writes.
  const href = scan?.running
    ? targetPath('sequence', db.dbId, scan.target_id, projectOf, null, {
        filterName: scan.filter_name,
      })
    : targetPath('sequence', db.dbId, backfill?.current_target_id, projectOf);
  const frames = scan ? `${verb} ${scan.processed}/${scan.total} frames` : '';
  // An automatic scan stepping aside for a person's work.
  const scanWaitsFor = scan?.running && scan.automatic ? scan.waiting_for ?? undefined : undefined;
  const scanItem = (key: string): ActivityItem | null => scan?.running
    ? {
        key,
        kind: 'quality',
        title: 'Analyzing quality',
        scope: db.dbName,
        detail: scanWaitsFor ? waitingDetail(scanWaitsFor, frames) : frames,
        status: scanWaitsFor ? 'waiting' : 'running',
        waitingFor: scanWaitsFor,
        percent: fraction(scan.processed, scan.total),
        background: !!scan.automatic,
        hint: scan.current_file ?? undefined,
        href,
      }
    : null;
  if (!backfill?.running) {
    const alone = scanItem(`scan:${db.dbId}`);
    return alone ? [alone] : [];
  }
  const targets = `${backfill.processed_targets}/${backfill.total_targets} targets`;
  const analysis: ActivityItem = {
    key: `backfill:${db.dbId}`,
    kind: 'quality',
    title: 'Analyzing database quality',
    scope: db.dbName,
    detail: targets,
    status: 'running',
    percent: fraction(backfill.processed_targets, backfill.total_targets),
    background: true,
    hint: scan?.current_file ?? undefined,
    href,
  };
  if (scan?.running && scan.automatic === false) {
    // A person's scan holds the scanner; the analysis waits its turn.
    const persons = scanItem(`scan:${db.dbId}`)!;
    const waitingFor = 'a quality scan';
    return [
      persons,
      {
        ...analysis,
        detail: waitingDetail(waitingFor, targets),
        status: 'waiting',
        waitingFor,
        hint: undefined,
        href: targetPath('sequence', db.dbId, backfill.current_target_id, projectOf),
      },
    ];
  }
  if (!scan?.running) return [analysis];
  return [{
    ...analysis,
    detail: scanWaitsFor ? waitingDetail(scanWaitsFor, `${targets} · ${frames}`) : `${targets} · ${frames}`,
    status: scanWaitsFor ? 'waiting' : 'running',
    waitingFor: scanWaitsFor,
  }];
}

/** A stretch, deconvolution or RC-Astro run on a finished stack. It waits in
 *  the same line as builds, so it moves and leaves the line the same way. */
function processingItem(entry: StackActivityEntry): ActivityItem {
  const queued = entry.state === 'queued';
  const processing = entry.processing;
  const fractionDone = processing?.fraction;
  // The title already names the operation; the line under it says what the
  // run waits for, or the step under way when the title does not, and when
  // it started, since a tool can take many minutes.
  const started = processing?.started_unix_seconds
    ? `started ${new Date(processing.started_unix_seconds * 1000).toLocaleTimeString([], {
      hour: '2-digit', minute: '2-digit',
    })}`
    : '';
  const step = processing?.stage && processing.stage !== processing.operation ? processing.stage : '';
  const detail = queued
    ? processing?.waiting_for ? `Waiting for ${processing.waiting_for}` : entry.detail
    : [step, started].filter(Boolean).join(' · ') || entry.detail;
  return {
    queue: 'stack',
    position: queued ? entry.queue_position ?? undefined : undefined,
    control: { kind: 'stack', jobId: entry.job_id },
    key: `stack:${entry.job_id}`,
    kind: 'processing',
    // A running stretch or deconvolution finishes in moments; an RC-Astro
    // tool can be stopped.
    stoppable: queued || !!processing?.stoppable,
    title: processing?.operation ?? 'Processing',
    scope: entry.label,
    detail,
    status: queued ? 'queued' : 'running',
    percent: !queued && fractionDone != null ? Math.min(fractionDone * 100, 100) : null,
    // Re-applied by the server after its stack refreshed.
    automatic: entry.automatic,
    background: entry.automatic,
    hint: processing?.waiting_for
      ? `${processing.operation} on ${entry.label}, waiting for ${processing.waiting_for}`
      : entry.detail,
    // The channel's card is on its target's stacks.
    href: viewPath('stacks', {
      db: entry.database_id,
      project: entry.project_id,
      target: entry.target_id,
    }),
  };
}

function stackItem(entry: StackActivityEntry): ActivityItem {
  if (entry.kind === 'processing') return processingItem(entry);
  const unit = entry.kind === 'mono' ? 'frames' : 'steps';
  const queued = entry.state === 'queued';
  return {
    queue: 'stack',
    position: queued ? entry.queue_position ?? undefined : undefined,
    control: { kind: 'stack', jobId: entry.job_id },
    key: `stack:${entry.job_id}`,
    kind: 'stack',
    stoppable: queued || entry.kind === 'mono',
    title: entry.kind === 'mono' ? 'Stacking' : 'Composing color',
    scope: entry.label,
    detail: queued
      ? 'Waiting for the build ahead'
      : entry.progress_label
        ?? (entry.total_units > 0
          ? `${entry.processed_units}/${entry.total_units} ${unit}`
          : entry.detail),
    status: queued ? 'queued' : 'running',
    percent: queued ? null : fraction(entry.processed_units, entry.total_units),
    automatic: entry.automatic,
    background: entry.automatic,
    hint: entry.detail,
    href: viewPath('stacks', {
      db: entry.database_id,
      project: entry.project_id,
      target: entry.target_id,
    }),
  };
}

const WBPP_STAGES: Record<string, string> = {
  planning: 'Planning the run',
  running: 'Running in PixInsight',
  publishing: 'Saving masters',
};

function wbppItems(wbpp: WbppActivity | undefined, projectOf: ProjectOf): ActivityItem[] {
  if (!wbpp) return [];
  // The stacks view carries a project's WBPP run and its outputs.
  const href = (dbId: string, projectId?: number | null, targetId?: number | null) =>
    targetPath('stacks', dbId, targetId, projectOf, projectId);
  const running = wbpp.running.map((run): ActivityItem => ({
    // The start time is part of the key, so the next run on the same
    // database is a new row and never inherits an armed Stop.
    key: `wbpp:${run.db_id}:${run.started_at ?? ''}`,
    stoppable: true,
    kind: 'wbpp',
    title: 'WBPP',
    scope: `${run.db_name} · ${run.scope}`,
    detail: [
      WBPP_STAGES[run.stage] ?? run.stage,
      run.wbpp_stage,
      run.wbpp_steps > 0 ? `${run.wbpp_steps} steps done` : '',
    ].filter(Boolean).join(' · '),
    status: 'running',
    percent: null,
    queue: 'wbpp',
    control: { kind: 'wbpp-running', dbId: run.db_id },
    href: href(run.db_id, run.project_id, run.target_id),
  }));
  const queued = wbpp.queued.map((run, index): ActivityItem => ({
    key: `wbpp-queued:${run.id}`,
    stoppable: true,
    kind: 'wbpp',
    title: 'WBPP',
    scope: `${run.db_name} · ${run.scope}`,
    detail: 'Waiting for the run ahead',
    status: 'queued',
    percent: null,
    queue: 'wbpp',
    position: index,
    control: { kind: 'wbpp-queued', dbId: run.db_id, queueId: run.id },
    href: href(run.db_id, run.project_id, run.target_id),
  }));
  return [...running, ...queued];
}

/** An automatic refresh that came due, or whose build stopped for a
 *  person's, waiting for the stacking worker. */
function waitingRefreshItem(refresh: ScheduledRefresh): ActivityItem {
  const last = refresh.last_progress;
  const stopped = last && last.total_units > 0
    ? `stopped at ${last.processed_units}/${last.total_units} frames`
    : '';
  const project = refresh.project_id == null
    ? 'every followed project'
    : refresh.project_name ?? `project ${refresh.project_id}`;
  const waitingFor = refresh.waiting_for ?? undefined;
  return {
    key: `refresh-waiting:${refresh.database_id}:${refresh.project_id ?? 'all'}`,
    kind: 'stack',
    title: 'Refreshing stacks',
    scope: `${refresh.database_name} · ${project}`,
    detail: waitingDetail(waitingFor, stopped),
    status: 'waiting',
    waitingFor,
    percent: last ? fraction(last.processed_units, last.total_units) : null,
    automatic: true,
    background: true,
    hint: refresh.channels?.length ? `Restacks ${refresh.channels.join(', ')}` : undefined,
    href: refresh.project_id == null
      ? viewPath('grid', { db: refresh.database_id })
      : viewPath('stacks', { db: refresh.database_id, project: refresh.project_id }),
  };
}

function waitingReapplyItem(reapply: WaitingReapply): ActivityItem {
  const waitingFor = reapply.waiting_for ?? undefined;
  return {
    key: `reapply-waiting:${reapply.database_id}:${reapply.project_id}:${reapply.label}`,
    kind: 'processing',
    title: 'Re-applying processing',
    scope: reapply.label,
    detail: waitingDetail(waitingFor),
    status: 'waiting',
    waitingFor,
    percent: null,
    automatic: true,
    background: true,
    href: viewPath('stacks', {
      db: reapply.database_id,
      project: reapply.project_id,
      target: reapply.target_id,
    }),
  };
}

/** A person's running work first, then running background work, then
 *  everything waiting: lines in the order they will run, then automatic
 *  work that stepped aside. */
function rank(item: ActivityItem): number {
  if (item.status !== 'running') return 2;
  return item.background ? 1 : 0;
}

/** Every job the header reports: running work, then waiting work. Automatic
 *  work that steps aside for a person's is listed as waiting. An automatic
 *  refresh still settling is not work yet; it shows once it is due. */
export function activityItems(
  databases: DatabaseActivity[],
  stacks: StackActivityEntry[],
  wbpp?: WbppActivity,
  projectOf: ProjectOf = () => undefined,
  waiting: WaitingStackWork = {}
): ActivityItem[] {
  const items: ActivityItem[] = [];
  for (const db of databases) {
    if (db.refresh?.is_refreshing) items.push(refreshItem(db, db.refresh));
    items.push(...qualityItems(db, projectOf));
  }
  // The server lists builds running first, then the line in order.
  const ordered = [...stacks].sort(
    (left, right) => Number(left.state === 'queued') - Number(right.state === 'queued')
      || (left.queue_position ?? Infinity) - (right.queue_position ?? Infinity)
  );
  items.push(...ordered.map(stackItem));
  items.push(...wbppItems(wbpp, projectOf));
  items.push(...(waiting.scheduled ?? []).filter((refresh) => refresh.waiting).map(waitingRefreshItem));
  items.push(...(waiting.reapplies ?? []).map(waitingReapplyItem));
  // Stable: each rank keeps the order above.
  return items.sort((left, right) => rank(left) - rank(right));
}

/** How many jobs wait in each line, for the move buttons' limits. */
export function lineLengths(items: ActivityItem[]): Record<ActivityQueue, number> {
  const lengths: Record<ActivityQueue, number> = { stack: 0, wbpp: 0 };
  for (const item of items) {
    if (item.queue && item.position != null) lengths[item.queue] += 1;
  }
  return lengths;
}

export interface ActivitySummary {
  count: number;
  running: number;
  /** Jobs queued in a line or waiting to resume. */
  waiting: number;
  /** The mean progress of the running jobs that report one, or null. */
  percent: number | null;
}

export function summarize(items: ActivityItem[]): ActivitySummary {
  const running = items.filter((item) => item.status === 'running');
  const measured = running.filter((item) => item.percent != null);
  return {
    count: items.length,
    running: running.length,
    waiting: items.length - running.length,
    percent: measured.length > 0
      ? measured.reduce((sum, item) => sum + (item.percent ?? 0), 0) / measured.length
      : null,
  };
}

/** The chip's count: `3 jobs` while all run, `1 running · 2 waiting` once
 *  any waits. */
export function countText(summary: ActivitySummary): string {
  if (summary.waiting === 0) return `${summary.count} job${summary.count === 1 ? '' : 's'}`;
  return `${summary.running} running · ${summary.waiting} waiting`;
}
