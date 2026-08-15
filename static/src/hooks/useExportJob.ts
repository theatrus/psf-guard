import { useEffect, useRef } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ExportJobProgress, ExportStatus } from '../api/types';

/**
 * Monitor the singleton per-DB server export job (started via
 * `apiClient.startServerExport`). Polls at 1s while the export runs.
 */
export function useExportJob(dbId: string | null | undefined) {
  const queryClient = useQueryClient();

  const statusQuery = useQuery<ExportStatus>({
    queryKey: ['db', dbId, 'export-job'],
    queryFn: () => apiClient.getServerExportStatus(dbId!),
    enabled: !!dbId,
    refetchInterval: (query) => (query.state.data?.progress.running ? 1000 : false),
    refetchIntervalInBackground: true,
  });

  const progress = statusQuery.data?.progress;
  const isRunning = progress?.running ?? false;

  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && !isRunning && dbId) {
      queryClient.invalidateQueries({ queryKey: ['db', dbId, 'export-job'] });
    }
    wasRunning.current = isRunning;
  }, [isRunning, dbId, queryClient]);

  return {
    progress,
    isRunning,
    refresh: statusQuery.refetch,
  };
}

/** One-line human description of an export job's current state. */
export function describeExportProgress(progress: ExportJobProgress | undefined): string | null {
  if (!progress || (!progress.running && !progress.finished_at)) return null;
  const scope = progress.scope ? ` ${progress.scope}` : '';
  switch (progress.stage) {
    case 'planning':
      return `Planning export of${scope}…`;
    case 'placing':
      return progress.total_files > 0
        ? `Exporting${scope}: ${progress.placed_files} of ${progress.total_files} files`
        : `Exporting${scope}…`;
    case 'scripts':
      return `Writing the WBPP runner…`;
    case 'error':
      return `Export failed: ${progress.error ?? 'unknown error'}`;
    case 'complete': {
      const outcome = progress.outcome;
      if (!outcome) return `Export of${scope} finished`;
      const placed = outcome.copied + outcome.linked + (outcome.reflinked ?? 0);
      const parts = [`${placed} file(s) placed`];
      if ((outcome.reflinked ?? 0) > 0) parts.push(`${outcome.reflinked} reflinked`);
      if (outcome.skipped_existing > 0) parts.push(`${outcome.skipped_existing} already present`);
      if (outcome.missing > 0) parts.push(`${outcome.missing} missing on disk`);
      if (outcome.errors > 0) parts.push(`${outcome.errors} ERRORS`);
      return `Exported${scope}: ${parts.join(', ')} → ${progress.destination}`;
    }
    default:
      return null;
  }
}
