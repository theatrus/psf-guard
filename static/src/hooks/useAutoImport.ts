import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { AutoImportStatus } from '../api/types';
import { noteImportStarted } from './useImportJob';

/**
 * The automatic import's settings, schedule and last run, polled quickly
 * while a run is under way and slowly otherwise so a scheduled run shows
 * up without a reload.
 */
export function useAutoImport(dbId: string | null | undefined) {
  const queryClient = useQueryClient();
  const queryKey = ['db', dbId, 'autoimport'] as const;
  const statusQuery = useQuery<AutoImportStatus>({
    queryKey,
    queryFn: () => apiClient.getAutoImportStatus(dbId!),
    enabled: !!dbId,
    refetchInterval: (query) =>
      query.state.data?.progress?.running ? 1000 : 30_000,
    refetchIntervalInBackground: true,
  });
  const runMutation = useMutation({
    mutationFn: () => apiClient.runAutoImport(dbId!),
    onSuccess: (status) => {
      noteImportStarted(queryClient, dbId!, status);
      queryClient.invalidateQueries({ queryKey });
    },
  });
  return {
    status: statusQuery.data,
    isRunning: statusQuery.data?.progress?.running ?? false,
    run: runMutation.mutate,
    isStarting: runMutation.isPending,
    error: runMutation.error ?? statusQuery.error,
  };
}
