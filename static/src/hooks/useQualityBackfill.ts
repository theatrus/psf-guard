import { useEffect, useRef } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { starMetadataFillEnabled } from './useStarMetadataFill';
import type { QualityBackfillStatus } from '../api/types';

export function useQualityBackfillStatus(dbId: string | null | undefined) {
  const queryClient = useQueryClient();
  const queryKey = ['db', dbId, 'quality-backfill'] as const;
  const statusQuery = useQuery<QualityBackfillStatus>({
    queryKey,
    queryFn: () => apiClient.getQualityBackfillStatus(dbId!),
    enabled: !!dbId,
    // A paused analysis changes only at Resume, which the header can send.
    refetchInterval: (query) => {
      const data = query.state.data;
      if (!data?.progress.running) return false;
      return data.paused ? 5000 : 1000;
    },
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

  return {
    status: statusQuery.data,
    isRunning,
    error: statusQuery.error,
  };
}

export function useQualityBackfill(dbId: string | null | undefined) {
  const queryClient = useQueryClient();
  const status = useQualityBackfillStatus(dbId);
  const queryKey = ['db', dbId, 'quality-backfill'] as const;

  const startMutation = useMutation({
    mutationFn: (force: boolean) =>
      apiClient.startQualityBackfill(dbId!, {
        force,
        fill_metadata: starMetadataFillEnabled(),
      }),
    onSuccess: (status) => queryClient.setQueryData(queryKey, status),
  });
  const resumeMutation = useMutation({
    mutationFn: () => apiClient.resumeJob(dbId!, 'quality'),
    onSettled: () => queryClient.invalidateQueries({ queryKey }),
  });
  const newFramesMutation = useMutation({
    mutationFn: (enabled: boolean) => apiClient.setAnalyzeNewFrames(dbId!, enabled),
    onSuccess: (status) => queryClient.setQueryData(queryKey, status),
  });

  return {
    ...status,
    isStarting: startMutation.isPending,
    error: startMutation.error ?? resumeMutation.error ?? newFramesMutation.error ?? status.error,
    start: startMutation.mutate,
    isPaused: status.status?.paused ?? false,
    resume: resumeMutation.mutate,
    isResuming: resumeMutation.isPending,
    analyzeNewFrames: status.status?.analyze_new_frames ?? false,
    setAnalyzeNewFrames: newFramesMutation.mutate,
    isSavingNewFrames: newFramesMutation.isPending,
  };
}
