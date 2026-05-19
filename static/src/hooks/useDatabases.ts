import { useQuery, useQueries } from '@tanstack/react-query';
import { useMemo } from 'react';
import { apiClient } from '../api/client';
import type {
  DatabaseSummary,
  ProjectOverview,
  TargetOverview,
  OverallStats,
} from '../api/types';

/** Each row returned by a merged hook carries the DB it came from. */
export type WithDb<T> = T & { db_id: string; db_name: string };

/** Hook returning every configured database. */
export function useAllDatabases() {
  return useQuery({
    queryKey: ['databases'],
    queryFn: apiClient.getDatabases,
    staleTime: 60_000,
  });
}

interface MergedResult<T> {
  data: WithDb<T>[];
  isLoading: boolean;
  isError: boolean;
  errors: unknown[];
}

/**
 * Fan-out per-DB query into a single merged list. Each row is stamped with
 * `db_id` and `db_name`.
 */
function useMergedPerDb<T>(
  databases: DatabaseSummary[] | undefined,
  queryKeyTail: string,
  fetcher: (dbId: string) => Promise<T[]>
): MergedResult<T> {
  const queries = useQueries({
    queries: (databases ?? []).map((db) => ({
      queryKey: ['db', db.id, queryKeyTail],
      queryFn: () => fetcher(db.id),
      staleTime: 30_000,
    })),
  });

  return useMemo(() => {
    const dbs = databases ?? [];
    const rows: WithDb<T>[] = [];
    let isLoading = false;
    let isError = false;
    const errors: unknown[] = [];

    queries.forEach((q, idx) => {
      const db = dbs[idx];
      if (!db) return;
      if (q.isLoading) isLoading = true;
      if (q.isError) {
        isError = true;
        errors.push(q.error);
      }
      const items = (q.data ?? []) as T[];
      for (const item of items) {
        rows.push({ ...(item as T), db_id: db.id, db_name: db.name });
      }
    });

    return { data: rows, isLoading, isError, errors };
  }, [queries, databases]);
}

/** Merged projects-overview across every configured database. */
export function useMergedProjectsOverview() {
  const { data: databases, isLoading: dbsLoading } = useAllDatabases();
  const merged = useMergedPerDb<ProjectOverview>(
    databases,
    'projects-overview',
    apiClient.getProjectsOverview
  );
  return {
    ...merged,
    isLoading: dbsLoading || merged.isLoading,
  };
}

/** Merged targets-overview across every configured database. */
export function useMergedTargetsOverview() {
  const { data: databases, isLoading: dbsLoading } = useAllDatabases();
  const merged = useMergedPerDb<TargetOverview>(
    databases,
    'targets-overview',
    apiClient.getTargetsOverview
  );
  return {
    ...merged,
    isLoading: dbsLoading || merged.isLoading,
  };
}

/**
 * Cross-DB overall stats. Sums numeric counters; unions sets like filter
 * names and merges date ranges. Returned `recent_activity` is the union
 * across DBs sorted by date desc, capped to 10 entries.
 */
export function useMergedOverallStats() {
  const { data: databases, isLoading: dbsLoading } = useAllDatabases();
  const queries = useQueries({
    queries: (databases ?? []).map((db) => ({
      queryKey: ['db', db.id, 'overall-stats'],
      queryFn: () => apiClient.getOverallStats(db.id),
      staleTime: 30_000,
    })),
  });

  return useMemo(() => {
    const isLoading = dbsLoading || queries.some((q) => q.isLoading);
    const stats: OverallStats[] = queries
      .map((q) => q.data)
      .filter((s): s is OverallStats => !!s);

    if (stats.length === 0) {
      return { data: undefined, isLoading };
    }

    const summed: OverallStats = {
      total_projects: 0,
      active_projects: 0,
      total_targets: 0,
      active_targets: 0,
      total_images: 0,
      accepted_images: 0,
      rejected_images: 0,
      pending_images: 0,
      total_desired: 0,
      files_found: 0,
      files_missing: 0,
      unique_filters: [],
      date_range: {},
      recent_activity: [],
    };

    const filterSet = new Set<string>();
    for (const s of stats) {
      summed.total_projects += s.total_projects;
      summed.active_projects += s.active_projects;
      summed.total_targets += s.total_targets;
      summed.active_targets += s.active_targets;
      summed.total_images += s.total_images;
      summed.accepted_images += s.accepted_images;
      summed.rejected_images += s.rejected_images;
      summed.pending_images += s.pending_images;
      summed.total_desired += s.total_desired;
      summed.files_found += s.files_found;
      summed.files_missing += s.files_missing;
      s.unique_filters.forEach((f) => filterSet.add(f));
      summed.recent_activity.push(...s.recent_activity);

      const a = summed.date_range;
      const b = s.date_range;
      if (b.earliest && (!a.earliest || b.earliest < a.earliest)) a.earliest = b.earliest;
      if (b.latest && (!a.latest || b.latest > a.latest)) a.latest = b.latest;
    }
    summed.unique_filters = Array.from(filterSet).sort();
    if (summed.date_range.earliest && summed.date_range.latest) {
      summed.date_range.span_days = Math.round(
        (summed.date_range.latest - summed.date_range.earliest) / 86_400
      );
    }
    summed.recent_activity.sort((a, b) => b.date - a.date);
    summed.recent_activity = summed.recent_activity.slice(0, 10);

    return { data: summed, isLoading };
  }, [queries, dbsLoading]);
}
