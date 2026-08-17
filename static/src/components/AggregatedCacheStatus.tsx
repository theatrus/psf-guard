import { useEffect, useState } from 'react';
import { useQueries, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  CacheRefreshProgress,
  QualityBackfillStatus,
  SpatialScanStatus,
} from '../api/types';
import { useAllDatabases } from '../hooks/useDatabases';
import './CacheRefreshStatus.css';

interface AggregatedCacheStatusProps {
  className?: string;
}

interface PerDbStatus {
  dbId: string;
  dbName: string;
  progress: CacheRefreshProgress | undefined;
  qualityScan: SpatialScanStatus | undefined;
  qualityBackfill: QualityBackfillStatus | undefined;
}

/**
 * Top-level cross-database job indicator, shown when the user is not scoped
 * to one database. It watches cache refresh, target quality scans, and
 * database quality backfills, then shows one summary with optional details.
 *
 * `DatabaseActivityStatus` handles the scoped case with richer progress.
 */
export default function AggregatedCacheStatus({
  className = '',
}: AggregatedCacheStatusProps) {
  const { data: databases = [] } = useAllDatabases();
  const queryClient = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const [previouslyActive, setPreviouslyActive] = useState<Set<string>>(new Set());

  const cacheQueries = useQueries({
    queries: databases.map((db) => ({
      queryKey: ['db', db.id, 'cache-progress'] as const,
      queryFn: () => apiClient.getCacheProgress(db.id),
      // Fast only while a refresh runs; a quiet server needs a slow
      // heartbeat, not a request per second per database.
      refetchInterval: (query: { state: { data?: CacheRefreshProgress } }) =>
        query.state.data?.is_refreshing ? 1000 : 10_000,
      refetchIntervalInBackground: false,
    })),
  });
  const qualityScanQueries = useQueries({
    queries: databases.map((db) => ({
      queryKey: ['db', db.id, 'quality-scan'] as const,
      queryFn: () => apiClient.getSpatialScanStatus(db.id),
      refetchInterval: (query: { state: { data?: SpatialScanStatus } }) =>
        query.state.data?.progress.running ? 1000 : false,
      refetchIntervalInBackground: false,
    })),
  });
  const qualityBackfillQueries = useQueries({
    queries: databases.map((db) => ({
      queryKey: ['db', db.id, 'quality-backfill'] as const,
      queryFn: () => apiClient.getQualityBackfillStatus(db.id),
      refetchInterval: (query: { state: { data?: QualityBackfillStatus } }) =>
        query.state.data?.progress.running ? 1000 : false,
      refetchIntervalInBackground: false,
    })),
  });

  const perDb: PerDbStatus[] = databases.map((db, idx) => ({
    dbId: db.id,
    dbName: db.name,
    progress: cacheQueries[idx]?.data,
    qualityScan: qualityScanQueries[idx]?.data,
    qualityBackfill: qualityBackfillQueries[idx]?.data,
  }));

  const activeDbs = perDb.filter((status) =>
    status.progress?.is_refreshing
      || status.qualityScan?.progress.running
      || status.qualityBackfill?.progress.running
  );
  const activeIds = activeDbs.map((status) => status.dbId);

  // When any database job finishes, invalidate that database so the merged
  // overview pulls fresh image and quality data.
  useEffect(() => {
    const currentSet = new Set(activeIds);
    const finished = [...previouslyActive].filter((id) => !currentSet.has(id));
    if (finished.length > 0) {
      finished.forEach((id) => {
        queryClient.invalidateQueries({ queryKey: ['db', id] });
      });
    }
    if (currentSet.size !== previouslyActive.size || finished.length > 0) {
      setPreviouslyActive(currentSet);
    }
  }, [activeIds, previouslyActive, queryClient]);

  if (activeDbs.length === 0) {
    return null;
  }

  const describeActivity = (status: PerDbStatus): string => {
    const backfill = status.qualityBackfill?.progress;
    const scan = status.qualityScan?.progress;
    if (backfill?.running) {
      const frameProgress = scan?.running
        ? ` · ${scan.stage === 'astrometry' ? 'solving' : 'scanning'} ${scan.processed}/${scan.total} frames`
        : '';
      return `quality ${backfill.processed_targets}/${backfill.total_targets} targets${frameProgress}`;
    }
    if (scan?.running) {
      return `quality ${scan.stage === 'astrometry' ? 'solve' : 'scan'} ${scan.processed}/${scan.total} frames`;
    }
    return status.progress?.current_project_name
      ?? status.progress?.current_directory_name
      ?? status.progress?.stage
      ?? 'working';
  };

  const activityPercentage = (status: PerDbStatus): number | null => {
    const backfill = status.qualityBackfill?.progress;
    if (backfill?.running && backfill.total_targets > 0) {
      return (backfill.processed_targets / backfill.total_targets) * 100;
    }
    const scan = status.qualityScan?.progress;
    if (scan?.running && scan.total > 0) {
      return (scan.processed / scan.total) * 100;
    }
    return status.progress && status.progress.progress_percentage > 0
      ? status.progress.progress_percentage
      : null;
  };

  return (
    <div
      className={`cache-refresh-status visible ${className}`}
      onClick={() => setExpanded((v) => !v)}
      style={{ cursor: 'pointer' }}
      title="Click to toggle per-database details"
    >
      <div className="cache-status-content">
        <div className="progress-indicator">
          <div className="pulsating-bar" />
        </div>
        <div className="cache-status-main">
          <div className="cache-status-label">
            Working on {activeDbs.length} of {databases.length} database
            {databases.length === 1 ? '' : 's'}
          </div>
          {expanded && (
            <div className="cache-status-details">
              {activeDbs.map((s) => {
                const percentage = activityPercentage(s);
                return (
                  <div key={s.dbId} className="progress-info-row">
                    <strong>{s.dbName}</strong>:{' '}
                    {describeActivity(s)}
                    {percentage != null && ` (${Math.round(percentage)}%)`}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
