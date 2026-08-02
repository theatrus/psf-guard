import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StackActivity, StackActivityEntry, StackActivityKind } from '../api/types';

export const STACK_ACTIVITY_QUERY_KEY = ['stack-activity'] as const;

/**
 * Stack builds still queued or running, across every database. Jobs live on
 * the server, so this survives navigation, panel unmount, and page reload.
 */
export function useStackActivity() {
  const query = useQuery<StackActivity>({
    queryKey: STACK_ACTIVITY_QUERY_KEY,
    queryFn: () => apiClient.getStackActivity(),
    refetchInterval: (query) => (query.state.data?.active.length ? 1000 : 3000),
    refetchIntervalInBackground: true,
  });
  return {
    active: query.data?.active ?? [],
    isRunning: (query.data?.active.length ?? 0) > 0,
  };
}

/**
 * The build a panel should re-attach to when it mounts without a job of its
 * own. Running work wins over queued work; ties go to the oldest job.
 */
export function adoptableStackJob(
  active: StackActivityEntry[],
  kind: StackActivityKind,
  dbId: string,
  projectId: number
): StackActivityEntry | undefined {
  const mine = active.filter(
    (entry) =>
      entry.kind === kind && entry.database_id === dbId && entry.project_id === projectId
  );
  return mine.find((entry) => entry.state === 'running') ?? mine[0];
}
