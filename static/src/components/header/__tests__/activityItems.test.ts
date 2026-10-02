import { describe, expect, it } from 'vitest';
import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  SpatialScanStatus,
  StackActivityEntry,
} from '../../../api/types';
import { activityItems, summarize } from '../activityItems';

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
  automatic = false
): StackActivityEntry {
  return {
    kind: 'mono', job_id: jobId, database_id: 'db-a', project_id: 1, state,
    label: `M44 · ${jobId}`, detail: 'registering', processed_units: processed,
    total_units: total, created_unix_seconds: 1, automatic,
  };
}

describe('header activity items', () => {
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
});
