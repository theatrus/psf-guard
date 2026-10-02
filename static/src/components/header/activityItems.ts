import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  SpatialScanStatus,
  StackActivityEntry,
} from '../../api/types';

/** One piece of background work the header reports. */
export interface ActivityItem {
  key: string;
  kind: 'refresh' | 'quality' | 'stack';
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
    key: `stack:${entry.job_id}`,
    kind: 'stack',
    title: entry.kind === 'mono' ? 'Stacking' : 'Composing color',
    scope: entry.label,
    detail: queued
      ? 'Waiting for the build ahead'
      : entry.total_units > 0
        ? `${entry.processed_units}/${entry.total_units} ${unit}`
        : entry.detail,
    queued,
    percent: queued ? null : fraction(entry.processed_units, entry.total_units),
    automatic: entry.automatic,
    hint: entry.detail,
  };
}

/** Every job the header reports, running work before queued work. */
export function activityItems(
  databases: DatabaseActivity[],
  stacks: StackActivityEntry[]
): ActivityItem[] {
  const items: ActivityItem[] = [];
  for (const db of databases) {
    if (db.refresh?.is_refreshing) items.push(refreshItem(db, db.refresh));
    const quality = qualityItem(db);
    if (quality) items.push(quality);
  }
  // Running builds before queued ones; the server lists each group oldest first.
  const ordered = [...stacks].sort(
    (left, right) => Number(left.state === 'queued') - Number(right.state === 'queued')
  );
  items.push(...ordered.map(stackItem));
  return items;
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
