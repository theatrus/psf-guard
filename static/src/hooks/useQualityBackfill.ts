import { useEffect, useRef } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { QualityBackfillStatus } from '../api/types';

export function useQualityBackfill(dbId: string) {
  const queryClient = useQueryClient();
  const queryKey = ['db', dbId, 'quality-backfill'] as const;
  const statusQuery = useQuery<QualityBackfillStatus>({
    queryKey,
    queryFn: () => apiClient.getQualityBackfillStatus(dbId),
    refetchInterval: (query) => (query.state.data?.progress.running ? 1000 : false),
    refetchIntervalInBackground: true,
  });
  const isRunning = statusQuery.data?.progress.running ?? false;
  const wasRunning = useRef(false);

  useEffect(() => {
    if (wasRunning.current && !isRunning) {
      queryClient.invalidateQueries({ queryKey: ['db', dbId] });
    }
    wasRunning.current = isRunning;
  }, [dbId, isRunning, queryClient]);

  const startMutation = useMutation({
    mutationFn: (force: boolean) => apiClient.startQualityBackfill(dbId, { force }),
    onSuccess: (status) => queryClient.setQueryData(queryKey, status),
  });

  return {
    status: statusQuery.data,
    isRunning,
    isStarting: startMutation.isPending,
    error: startMutation.error ?? statusQuery.error,
    start: startMutation.mutate,
  };
}
