import { useEffect, useRef } from 'react';
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { WbppQueuedRun, WbppRunProgress, WbppRunStatus } from '../api/types';

function pollInterval(status: WbppRunStatus | undefined): number | false {
  const progress = status?.progress;
  if (progress?.running || progress?.publish?.state === 'running') return 2000;
  // A run in line starts on its own; poll so the switch shows up.
  if ((status?.queued?.length ?? 0) > 0) return 5000;
  return false;
}

/**
 * Follow a database's WBPP run (started with `apiClient.startWbppRun`) and
 * its runs waiting in line. Polls every 2 s while PixInsight runs.
 */
export function useWbppRun(dbId: string | null | undefined) {
  const queryClient = useQueryClient();
  const statusQuery = useQuery<WbppRunStatus>({
    queryKey: ['db', dbId, 'wbpp-run'],
    queryFn: () => apiClient.getWbppRun(dbId!),
    enabled: !!dbId,
    refetchInterval: (query) => pollInterval(query.state.data),
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

  return {
    status: statusQuery.data,
    progress,
    queued: statusQuery.data?.queued ?? [],
    isRunning,
    refresh: statusQuery.refetch,
  };
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

/** "1st", "2nd", "3rd", "4th". */
export function ordinal(position: number): string {
  const rest = position % 100;
  if (rest >= 11 && rest <= 13) return `${position}th`;
  switch (position % 10) {
    case 1:
      return `${position}st`;
    case 2:
      return `${position}nd`;
    case 3:
      return `${position}rd`;
    default:
      return `${position}th`;
  }
}

/** One line for a run waiting its turn. */
export function describeQueuedRun(entry: WbppQueuedRun): string {
  return entry.position === 1
    ? `WBPP ${entry.scope} is next in line`
    : `WBPP ${entry.scope} is ${ordinal(entry.position)} in line`;
}

/** The tone a run's line takes: what its colour says. */
export function wbppRunTone(progress: WbppRunProgress): 'running' | 'done' | 'error' {
  if (progress.running) return 'running';
  return progress.stage === 'error' ? 'error' : 'done';
}

/** Whether a run is one the Overview should still show. */
export function isRunOfInterest(progress: WbppRunProgress | undefined): progress is WbppRunProgress {
  return !!progress && (progress.running || !!progress.finished_at);
}

/**
 * The current WBPP run and the waiting runs of every database, so the
 * Overview can show them in any browser tab, not only the one that started
 * them. A database is in the map when it has a run to show or runs in line.
 */
export function useWbppRuns(dbIds: string[]): Map<string, WbppRunStatus> {
  const results = useQueries({
    queries: dbIds.map((dbId) => ({
      queryKey: ['db', dbId, 'wbpp-run'],
      queryFn: () => apiClient.getWbppRun(dbId),
      staleTime: 5_000,
      refetchInterval: (query: { state: { data?: WbppRunStatus } }) =>
        pollInterval(query.state.data),
      refetchIntervalInBackground: true,
    })),
  });
  const runs = new Map<string, WbppRunStatus>();
  results.forEach((result, index) => {
    const status = result.data;
    if (!status) return;
    // An older server answers without the queue.
    const queued = status.queued ?? [];
    if (isRunOfInterest(status.progress) || queued.length > 0) {
      runs.set(dbIds[index], { ...status, queued });
    }
  });
  return runs;
}

/** Whether a run is the one this project asked for. */
export function isProjectsRun(progress: WbppRunProgress | undefined, projectId: number): boolean {
  return isRunOfInterest(progress) && progress.project_id === projectId;
}

/** The short state a project card shows: its own run, or its place in line. */
export function describeWbppRunForProject(
  status: WbppRunStatus | undefined,
  projectId: number
): { label: string; tone: 'running' | 'done' | 'error' | 'queued' } | null {
  if (!status) return null;
  const queued = (status.queued ?? []).find((entry) => entry.project_id === projectId);
  if (queued) return { label: 'Queued for WBPP', tone: 'queued' };
  const progress = status.progress;
  if (!isProjectsRun(progress, projectId)) return null;
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
