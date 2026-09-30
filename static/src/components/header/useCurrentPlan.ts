import { useMemo } from 'react';
import { useLocation, useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useDbProjectTarget } from '../../hooks/useUrlState';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { retryWhenBusy } from '../director/retry';

/** The plan in scope and the way to its workspace. A plan is in scope when
 *  the URL names one (`directorProject`) or when the review scope's project
 *  is one of its rigs, so picking a project for review also picks its plan. */
export function useCurrentPlan() {
  const director = useDirectorStatus();
  const enabled = !!director.data?.enabled && director.data.protocol_version === 1 && !!director.data.instance_id;
  const instanceId = director.data?.instance_id ?? 'none';
  const plans = useQuery({ queryKey: ['directorPlans', instanceId], queryFn: apiClient.getDirectorPlans, enabled, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false, staleTime: 30_000 });
  const [params] = useSearchParams();
  const { dbId, projectId } = useDbProjectTarget();
  const rows = useMemo(() => plans.data?.rows ?? [], [plans.data]);
  // Only the Planning page names a plan; elsewhere the review scope decides.
  const { pathname } = useLocation();
  const named = pathname === '/director' ? params.get('directorProject') : null;
  const current = useMemo(() => {
    if (named) return rows.find(row => row.project.id === named) ?? null;
    if (dbId && projectId !== null) return rows.find(row => row.links.some(link => link.catalog_slug === dbId && link.source_row_id === projectId)) ?? null;
    return null;
  }, [rows, named, dbId, projectId]);
  // The workspace keeps the page's other scope so the Library returns where it was.
  const hrefFor = (planId: string) => {
    const next = new URLSearchParams(params);
    next.delete('directorSource'); next.delete('directorView'); next.delete('directorCatalog');
    next.set('directorProject', planId);
    return `/director?${next}`;
  };
  return { enabled, rows, current, hrefFor, loading: enabled && plans.isPending, error: plans.isError };
}
