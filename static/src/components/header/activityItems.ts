import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  ScheduledRefresh,
  SpatialScanStatus,
  StackActivityEntry,
  WbppActivity,
} from '../../api/types';

/** What a row can do: stop its job, and move it in its line. */
export type ActivityControl =
  | { kind: 'stack'; jobId: string }
  | { kind: 'scheduled'; dbId: string; projectId: number | null }
  | { kind: 'wbpp-running'; dbId: string }
  | { kind: 'wbpp-queued'; dbId: string; queueId: string };

/** The two lines a person can reorder. Each runs one job at a time. */
export type ActivityQueue = 'stack' | 'wbpp';

/** One piece of background work the header reports. */
export interface ActivityItem {
  key: string;
  kind: 'refresh' | 'quality' | 'stack' | 'wbpp' | 'automatic';
  /** What is happening: "Stacking", "Analyzing quality". */
  title: string;
  /** What it happens to: a database, or a target and channel. */
  scope: string;
  /** The progress in words: "1/3 frames", a stage, a file. */
  detail: string;
  queued: boolean;
  /** 0–100, or null when the work cannot say how far along it is. */
  percent: number | null;
  automatic?: boolean;
  /** The full current path or file, for a tooltip. */
  hint?: string;
  /** The line this job runs in, when it has one. */
  queue?: ActivityQueue;
  /** Place among the waiting jobs of its line, 0 next. */
  position?: number;
  control?: ActivityControl;
  /** Whether Stop can end it while it runs. A color composition cannot. */
  stoppable?: boolean;
  /** What the state corner says of a waiting job, such as when it starts. */
  state?: string;
}

export interface DatabaseActivity {
  dbId: string;
  dbName: string;
  refresh?: CacheRefreshProgress;
  scan?: SpatialScanStatus;
  backfill?: QualityBackfillStatus;
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
    queued: false,
    percent: refresh.progress_percentage > 0 ? Math.min(refresh.progress_percentage, 100) : null,
    hint: refresh.current_directory_name ?? refresh.current_project_name ?? undefined,
  };
}

function qualityItem(db: DatabaseActivity): ActivityItem | null {
  const backfill = db.backfill?.progress;
  const scan = db.scan?.progress;
  const verb = scan?.stage === 'astrometry' ? 'Solving' : 'Scanning';
  if (backfill?.running) {
    const frames = scan?.running ? ` · ${verb} ${scan.processed}/${scan.total} frames` : '';
    return {
      key: `backfill:${db.dbId}`,
      kind: 'quality',
      title: 'Analyzing database quality',
      scope: db.dbName,
      detail: `${backfill.processed_targets}/${backfill.total_targets} targets${frames}`,
      queued: false,
      percent: fraction(backfill.processed_targets, backfill.total_targets),
      hint: scan?.current_file ?? undefined,
    };
  }
  if (scan?.running) {
    return {
      key: `scan:${db.dbId}`,
      kind: 'quality',
      title: 'Analyzing quality',
      scope: db.dbName,
      detail: `${verb} ${scan.processed}/${scan.total} frames`,
      queued: false,
      percent: fraction(scan.processed, scan.total),
      hint: scan.current_file ?? undefined,
    };
  }
  return null;
}

function stackItem(entry: StackActivityEntry): ActivityItem {
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
    queued,
    percent: queued ? null : fraction(entry.processed_units, entry.total_units),
    automatic: entry.automatic,
    hint: entry.detail,
  };
}

const WBPP_STAGES: Record<string, string> = {
  planning: 'Planning the run',
  running: 'Running in PixInsight',
  publishing: 'Saving masters',
};

function wbppItems(wbpp: WbppActivity | undefined): ActivityItem[] {
  if (!wbpp) return [];
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
    queued: false,
    percent: null,
    queue: 'wbpp',
    control: { kind: 'wbpp-running', dbId: run.db_id },
  }));
  const queued = wbpp.queued.map((run, index): ActivityItem => ({
    key: `wbpp-queued:${run.id}`,
    stoppable: true,
    kind: 'wbpp',
    title: 'WBPP',
    scope: `${run.db_name} · ${run.scope}`,
    detail: 'Waiting for the run ahead',
    queued: true,
    percent: null,
    queue: 'wbpp',
    position: index,
    control: { kind: 'wbpp-queued', dbId: run.db_id, queueId: run.id },
  }));
  return [...running, ...queued];
}

const REFRESH_REASONS: Record<ScheduledRefresh['reason'], string> = {
  arrival: 'new frames',
  sync: 'a sync',
  grade: 'grade changes',
};

function startsIn(seconds: number): string {
  if (seconds <= 15) return 'starting now';
  const minutes = Math.ceil(seconds / 60);
  return `in ${minutes} min`;
}

/** `T · R, T · G (new) +2 more`: what a waiting refresh will stack. */
function channelsText(channels: string[]): string {
  const shown = channels.slice(0, 3).join(', ');
  return channels.length > 3 ? `${shown} +${channels.length - 3} more` : shown;
}

function scheduledItems(scheduled: ScheduledRefresh[]): ActivityItem[] {
  return scheduled.map((refresh) => ({
    key: `scheduled:${refresh.database_id}:${refresh.project_id ?? 'all'}`,
    kind: 'automatic',
    title: 'Automatic refresh',
    scope: `${refresh.database_name} · ${
      refresh.project_id == null
        ? 'every followed project'
        : refresh.project_name ?? `project ${refresh.project_id}`
    } · after ${REFRESH_REASONS[refresh.reason]}`,
    detail: refresh.channels?.length ? `Restacks ${channelsText(refresh.channels)}` : 'Restacks what changed',
    hint: refresh.channels?.join('\n'),
    state: startsIn(refresh.due_in_seconds),
    queued: true,
    percent: null,
    automatic: true,
    control: { kind: 'scheduled', dbId: refresh.database_id, projectId: refresh.project_id },
  }));
}

/** Every job the header reports: each line's running work, then its waiting
 *  work in the order it will run, then automatic refreshes still settling. */
export function activityItems(
  databases: DatabaseActivity[],
  stacks: StackActivityEntry[],
  wbpp?: WbppActivity,
  scheduled: ScheduledRefresh[] = []
): ActivityItem[] {
  const items: ActivityItem[] = [];
  for (const db of databases) {
    if (db.refresh?.is_refreshing) items.push(refreshItem(db, db.refresh));
    const quality = qualityItem(db);
    if (quality) items.push(quality);
  }
  // The server lists builds running first, then the line in order.
  const ordered = [...stacks].sort(
    (left, right) => Number(left.state === 'queued') - Number(right.state === 'queued')
      || (left.queue_position ?? Infinity) - (right.queue_position ?? Infinity)
  );
  items.push(...ordered.map(stackItem));
  items.push(...wbppItems(wbpp));
  items.push(...scheduledItems(scheduled));
  return items;
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
  queued: number;
  /** The mean progress of the running jobs that report one, or null. */
  percent: number | null;
}

export function summarize(items: ActivityItem[]): ActivitySummary {
  const measured = items.filter((item) => !item.queued && item.percent != null);
  return {
    count: items.length,
    queued: items.filter((item) => item.queued).length,
    percent: measured.length > 0
      ? measured.reduce((sum, item) => sum + (item.percent ?? 0), 0) / measured.length
      : null,
  };
}
