import { useEffect, useRef } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ImportStatus } from '../api/types';

/**
 * Monitor the singleton per-DB FITS import job (started via
 * `apiClient.createDatabaseFromImages` or `apiClient.startImport`).
 *
 * Polls at 1s while the job runs (same pattern as useSpatialScan). When the
 * job transitions running → finished, every per-DB query plus the databases
 * listing is invalidated so overviews pick up the imported projects; the
 * backfill stage additionally feeds sequence-analysis, which is covered by
 * the blanket per-DB invalidation.
 */
export function useImportJob(dbId: string | null | undefined) {
  const queryClient = useQueryClient();

  const statusQuery = useQuery<ImportStatus>({
    queryKey: ['db', dbId, 'import-job'],
    queryFn: () => apiClient.getImportStatus(dbId!),
    enabled: !!dbId,
    refetchInterval: (query) => (query.state.data?.progress.running ? 1000 : false),
    refetchIntervalInBackground: true,
  });

  const progress = statusQuery.data?.progress;
  const isRunning = progress?.running ?? false;

  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && !isRunning && dbId) {
      queryClient.invalidateQueries({ queryKey: ['databases'] });
      queryClient.invalidateQueries({ queryKey: ['db', dbId] });
    }
    wasRunning.current = isRunning;
  }, [isRunning, dbId, queryClient]);

  return {
    status: statusQuery.data,
    progress,
    isRunning,
  };
}

/** One-line human description of an import job's current state. */
export function describeImportProgress(
  progress: import('../api/types').ImportJobProgress | undefined
): string {
  if (!progress || progress.stage === '') return '';
  switch (progress.stage) {
    case 'scanning':
      return `Scanning headers… ${progress.scanned_files}/${progress.total_files}`;
    case 'importing':
      return `Importing ${progress.total_files} frame(s) into the database…`;
    case 'backfill':
      return `Analyzing quality… target ${progress.backfill_done + 1}/${progress.backfill_total}`;
    case 'complete': {
      const o = progress.outcome;
      if (!o) return 'Import complete.';
      const skipped = o.skipped_existing > 0 ? `, ${o.skipped_existing} already present` : '';
      return o.dry_run
        ? `Dry run: would import ${o.imported} frame(s) into ${o.projects_created} project(s)${skipped}.`
        : `Imported ${o.imported} frame(s) into ${o.projects_created} project(s), ${o.targets_created} target(s)${skipped}.`;
    }
    case 'error':
      return `Import failed: ${progress.error ?? 'unknown error'}`;
    default:
      return progress.stage;
  }
}
