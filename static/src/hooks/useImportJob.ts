import { useEffect, useRef } from 'react';
import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ImportStatus } from '../api/types';

/**
 * Hand a job that just started to the poller. The poll interval switches
 * off once a job reports finished, so a preview that completed leaves the
 * query idle; without seeding it, the real import that follows is never
 * polled and the page keeps showing the preview's final state.
 */
export function noteImportStarted(queryClient: QueryClient, dbId: string, status: ImportStatus) {
  queryClient.setQueryData<ImportStatus>(['db', dbId, 'import-job'], status);
  queryClient.invalidateQueries({ queryKey: ['db', dbId, 'import-job'] });
}

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
      const calibrationPart =
        calibrationChanged > 0
          ? `${calibrationChanged} calibration frame(s)`
          : calibration.skipped_existing > 0
            ? `${calibration.skipped_existing} calibration frame(s) unchanged`
            : '';
      // A calibration-only import leads with its calibration frames instead
      // of "0 light frame(s) — nothing new".
      const calibrationOnly = o.imported === 0 && calibrationChanged > 0;
      const lightPart = calibrationOnly
        ? o.skipped_existing > 0
          ? `${o.skipped_existing} light frame(s) already present`
          : ''
        : `${o.imported} light frame(s) — ${detail}${skipped}`;
      const summary = (calibrationOnly ? [calibrationPart, lightPart] : [lightPart, calibrationPart])
        .filter(Boolean)
        .join(', ');
      return o.dry_run ? `Preview: would import ${summary}.` : `Imported ${summary}.`;
    }
    case 'error':
      return `Import failed: ${progress.error ?? 'unknown error'}`;
    default:
      return progress.stage;
  }
}

/**
 * Footer message for an import job that just stopped running, or `null`
 * while it runs. A preview hands over to the confirm step below it; a
 * real import reports what it wrote; a failure reads as one.
 */
export function importFinishedMessage(
  progress: import('../api/types').ImportJobProgress | undefined,
  dbName: string
): string | null {
  if (!progress || progress.running) return null;
  switch (progress.stage) {
    case 'error':
      return describeImportProgress(progress);
    case 'complete':
      return progress.outcome?.dry_run
        ? `Preview ready for ${dbName} — nothing is written until you confirm.`
        : describeImportProgress(progress);
    default:
      return null;
  }
}
