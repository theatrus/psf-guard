import { useCallback, useMemo } from 'react';
import { useLocation, useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DirectorPlanRow } from '../../api/directorTypes';
import { useDbProjectTarget } from '../../hooks/useUrlState';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { retryWhenBusy } from '../director/retry';

/** Every plan, once for the whole page: the header's plan picker, the
 *  Library's plan links and its plans without a database share this query.
 *  Nothing is asked when Director is off. */
export function usePlans() {
  const director = useDirectorStatus();
  const enabled = !!director.data?.enabled && director.data.protocol_version === 1 && !!director.data.instance_id;
  const instanceId = director.data?.instance_id ?? 'none';
  const query = useQuery({ queryKey: ['directorPlans', instanceId], queryFn: apiClient.getDirectorPlans, enabled, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false, staleTime: 30_000 });
  const [params] = useSearchParams();
  const rows = useMemo(() => query.data?.rows ?? [], [query.data]);
  // A database's project row belongs to at most one plan.
  const byProject = useMemo(() => {
    const map = new Map<string, DirectorPlanRow>();
    for (const row of rows) for (const link of row.links) if (link.source_row_id !== null) map.set(`${link.catalog_slug}:${link.source_row_id}`, row);
    return map;
  }, [rows]);
  const planFor = useCallback((dbId: string, projectId: number) => byProject.get(`${dbId}:${projectId}`) ?? null, [byProject]);
  // The workspace keeps the page's other scope so the Library returns where it was.
  const hrefFor = (planId: string) => {
    const next = new URLSearchParams(params);
    next.delete('directorSource'); next.delete('directorView'); next.delete('directorCatalog');
    next.set('directorProject', planId);
    return `/director?${next}`;
  };
  return { enabled, instanceId, query, rows, planFor, hrefFor, loading: enabled && query.isPending, error: query.isError };
}

/** The plan in scope and the way to its workspace. A plan is in scope when
 *  the Planning page names one (`directorProject`) or when the review scope's
 *  project is one of its rigs, so picking a project for review also picks
 *  its plan. */
export function useCurrentPlan() {
  const plans = usePlans();
  const [params] = useSearchParams();
  const { dbId, projectId } = useDbProjectTarget();
  // Only the Planning page names a plan; elsewhere the review scope decides.
  const { pathname } = useLocation();
  const named = pathname === '/director' ? params.get('directorProject') : null;
  const { rows, planFor } = plans;
  const current = useMemo(() => {
    if (named) return rows.find(row => row.project.id === named) ?? null;
    if (dbId && projectId !== null) return planFor(dbId, projectId);
    return null;
  }, [rows, named, dbId, projectId, planFor]);
  return { enabled: plans.enabled, rows, current, hrefFor: plans.hrefFor, loading: plans.loading, error: plans.error };
}
