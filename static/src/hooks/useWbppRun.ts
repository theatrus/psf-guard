import { useEffect, useRef } from 'react';
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { WbppRunProgress, WbppRunStatus } from '../api/types';

/**
 * Follow the singleton per-database WBPP run (started with
 * `apiClient.startWbppRun`). Polls every 2 s while PixInsight runs.
 */
export function useWbppRun(dbId: string | null | undefined) {
  const queryClient = useQueryClient();
  const statusQuery = useQuery<WbppRunStatus>({
    queryKey: ['db', dbId, 'wbpp-run'],
    queryFn: () => apiClient.getWbppRun(dbId!),
    enabled: !!dbId,
    refetchInterval: (query) => {
      const progress = query.state.data?.progress;
      return progress?.running || progress?.publish?.state === 'running' ? 2000 : false;
    },
    refetchIntervalInBackground: true,
  });

  const progress = statusQuery.data?.progress;
  const isRunning = progress?.running ?? false;
  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && !isRunning && dbId) {
      queryClient.invalidateQueries({ queryKey: ['db', dbId, 'wbpp-run'] });
    }
    wasRunning.current = isRunning;
  }, [isRunning, dbId, queryClient]);

  return { progress, isRunning, refresh: statusQuery.refetch };
}

/** One line on where a WBPP run stands, or null before any run. */
export function describeWbppRun(progress: WbppRunProgress | undefined): string | null {
  if (!progress || (!progress.running && !progress.finished_at)) return null;
  const scope = progress.scope ? ` ${progress.scope}` : '';
  switch (progress.stage) {
    case 'planning':
      return `WBPP: planning${scope}…`;
    case 'launching':
      return `WBPP: starting PixInsight for${scope} (${progress.lights} lights)…`;
    case 'running':
      return progress.wbpp_stage
        ? `WBPP${scope}: ${progress.wbpp_stage}`
        : `WBPP${scope}: PixInsight is running…`;
    case 'complete': {
      const masters = progress.outputs.filter((file) => file.kind === 'master').length;
      return `WBPP${scope} finished: ${masters} master${masters === 1 ? '' : 's'}${
        progress.wbpp_elapsed ? ` in ${progress.wbpp_elapsed}` : ''
      }`;
    }
    case 'cancelled':
      return `WBPP${scope} was stopped`;
    case 'error':
      return `WBPP${scope} failed: ${progress.error ?? 'unknown error'}`;
    default:
      return null;
  }
}

/** Whether a run is one the Overview should still show. */
export function isRunOfInterest(progress: WbppRunProgress | undefined): progress is WbppRunProgress {
  return !!progress && (progress.running || !!progress.finished_at);
}

/**
 * The current WBPP run of every database, so the Overview can show a run
 * under way or just finished in any browser tab, not only the one that
 * started it. Polls the databases whose run is under way.
 */
export function useWbppRuns(dbIds: string[]): Map<string, WbppRunProgress> {
  const results = useQueries({
    queries: dbIds.map((dbId) => ({
      queryKey: ['db', dbId, 'wbpp-run'],
      queryFn: () => apiClient.getWbppRun(dbId),
      staleTime: 5_000,
      refetchInterval: (query: { state: { data?: WbppRunStatus } }) => {
        const progress = query.state.data?.progress;
        return progress?.running || progress?.publish?.state === 'running' ? 2000 : false;
      },
      refetchIntervalInBackground: true,
    })),
  });
  const runs = new Map<string, WbppRunProgress>();
  results.forEach((result, index) => {
    const progress = result.data?.progress;
    if (isRunOfInterest(progress)) runs.set(dbIds[index], progress);
  });
  return runs;
}

/** The short state a project card shows for its database's run, if the run is its own. */
export function describeWbppRunForProject(
  progress: WbppRunProgress | undefined,
  projectId: number
): { label: string; tone: 'running' | 'done' | 'error' } | null {
  if (!isRunOfInterest(progress) || progress.project_id !== projectId) return null;
  if (progress.running) return { label: 'Stacking in WBPP…', tone: 'running' };
  switch (progress.stage) {
    case 'complete':
      return { label: 'WBPP masters ready', tone: 'done' };
    case 'error':
      return { label: 'WBPP failed', tone: 'error' };
    default:
      return null;
  }
}
