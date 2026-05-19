import { useEffect, useState } from 'react';
import { useQueries, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { CacheRefreshProgress } from '../api/types';
import { useAllDatabases } from '../hooks/useDatabases';
import './CacheRefreshStatus.css';

interface AggregatedCacheStatusProps {
  className?: string;
}

interface PerDbStatus {
  dbId: string;
  dbName: string;
  progress: CacheRefreshProgress | undefined;
}

/**
 * Top-level cross-DB cache refresh indicator, shown when the user isn't
 * scoped to a single database (i.e. the URL has no `?db=`). Polls every
 * configured database's `/cache-progress` endpoint in parallel and surfaces
 * a single-line summary; click to expand for per-DB details.
 *
 * The scoped-view counterpart (`CacheRefreshStatus`) handles the
 * single-database case with its richer per-directory progress display.
 */
export default function AggregatedCacheStatus({
  className = '',
}: AggregatedCacheStatusProps) {
  const { data: databases = [] } = useAllDatabases();
  const queryClient = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const [previouslyRefreshing, setPreviouslyRefreshing] = useState<Set<string>>(new Set());

  const queries = useQueries({
    queries: databases.map((db) => ({
      queryKey: ['db', db.id, 'cache-progress'] as const,
      queryFn: () => apiClient.getCacheProgress(db.id),
      refetchInterval: 1000,
      refetchIntervalInBackground: true,
    })),
  });

  const perDb: PerDbStatus[] = databases.map((db, idx) => ({
    dbId: db.id,
    dbName: db.name,
    progress: queries[idx]?.data,
  }));

  const refreshingDbs = perDb.filter((s) => s.progress?.is_refreshing);
  const refreshingIds = refreshingDbs.map((s) => s.dbId);

  // When any DB transitions from refreshing → idle, invalidate that DB's
  // query cache so the merged-overview hooks pull fresh data.
  useEffect(() => {
    const currentSet = new Set(refreshingIds);
    const finished = [...previouslyRefreshing].filter((id) => !currentSet.has(id));
    if (finished.length > 0) {
      finished.forEach((id) => {
        queryClient.invalidateQueries({ queryKey: ['db', id] });
      });
    }
    if (currentSet.size !== previouslyRefreshing.size || finished.length > 0) {
      setPreviouslyRefreshing(currentSet);
    }
  }, [refreshingIds, previouslyRefreshing, queryClient]);

  if (refreshingDbs.length === 0) {
    return null;
  }

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
            Refreshing {refreshingDbs.length} of {databases.length} database
            {databases.length === 1 ? '' : 's'}
          </div>
          {expanded && (
            <div className="cache-status-details">
              {refreshingDbs.map((s) => (
                <div key={s.dbId} className="progress-info-row">
                  <strong>{s.dbName}</strong>:{' '}
                  {s.progress?.current_project_name
                    ?? s.progress?.current_directory_name
                    ?? s.progress?.stage
                    ?? '…'}
                  {s.progress &&
                    s.progress.progress_percentage > 0 &&
                    ` (${Math.round(s.progress.progress_percentage)}%)`}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
