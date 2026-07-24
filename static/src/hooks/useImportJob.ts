import { useEffect, useRef } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ImportStatus } from '../api/types';

/**
 * Monitor the singleton per-DB FITS import job (started via
 * `apiClient.createDatabaseFromImages` or `apiClient.startImport`).
 *
 * Polls at 1s while the import runs and invalidates database views when it
 * completes. Database-wide quality work has its own job and hook.
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
    case 'complete': {
      const o = progress.outcome;
      if (!o) return 'Import complete.';
      const skipped = o.skipped_existing > 0 ? `, ${o.skipped_existing} already present` : '';
      const attached = o.attached > 0 ? `${o.attached} to existing target(s)` : '';
      const fresh =
        o.projects_created > 0
          ? `${o.imported - o.attached} into ${o.projects_created} NEW project(s)`
          : '';
      const detail = [attached, fresh].filter(Boolean).join(', ') || 'nothing new';
      const calibration = o.calibration;
      const calibrationChanged = calibration.imported + calibration.updated;
      const calibrationText =
        calibrationChanged > 0
          ? `, ${calibrationChanged} calibration frame(s)`
          : calibration.skipped_existing > 0
            ? `, ${calibration.skipped_existing} calibration frame(s) unchanged`
            : '';
      return o.dry_run
        ? `Preview: would import ${o.imported} light frame(s) — ${detail}${skipped}${calibrationText}.`
        : `Imported ${o.imported} light frame(s) — ${detail}${skipped}${calibrationText}.`;
    }
    case 'error':
      return `Import failed: ${progress.error ?? 'unknown error'}`;
    default:
      return progress.stage;
  }
}
