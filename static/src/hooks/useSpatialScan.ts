import { useEffect, useRef } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { SpatialScanStatus } from '../api/types';

/** Monitor the database's quality scan from any view. */
export function useSpatialScanStatus(dbId: string | null | undefined) {
  const queryClient = useQueryClient();
  const statusQuery = useQuery<SpatialScanStatus>({
    queryKey: ['db', dbId, 'quality-scan'],
    queryFn: () => apiClient.getSpatialScanStatus(dbId!),
    enabled: !!dbId,
    refetchInterval: (query) => (query.state.data?.progress.running ? 1000 : false),
    refetchIntervalInBackground: true,
  });

  const isRunning = statusQuery.data?.progress.running ?? false;

  // When a scan transitions running -> finished, refresh the analysis so the
  // new metrics show up.
  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && !isRunning) {
      queryClient.invalidateQueries({ queryKey: ['db', dbId, 'sequence-analysis'] });
      queryClient.invalidateQueries({ queryKey: ['db', dbId, 'image-quality'] });
    }
    wasRunning.current = isRunning;
  }, [isRunning, dbId, queryClient]);

  return {
    status: statusQuery.data,
    statusError: statusQuery.error,
    isRunning,
  };
}

/** Start a target scan and share its progress with global status consumers. */
export function useSpatialScan(
  dbId: string | null | undefined,
  targetId: number | undefined,
  filterName?: string
) {
  const queryClient = useQueryClient();
  const status = useSpatialScanStatus(dbId);

  const startMutation = useMutation({
    mutationFn: (force?: boolean) =>
      apiClient.startSpatialScan(dbId!, {
        target_id: targetId!,
        filter_name: filterName,
        force,
      }),
    onSuccess: (status) => {
      // Seed the poll query so refetchInterval kicks in immediately.
      queryClient.setQueryData(['db', dbId, 'quality-scan'], status);
      if (!status.started && !status.progress.running) {
        // Nothing needed computing; metrics may still be newly relevant.
        queryClient.invalidateQueries({ queryKey: ['db', dbId, 'sequence-analysis'] });
      }
    },
  });

  return {
    ...status,
    start: startMutation.mutate,
    isStarting: startMutation.isPending,
    startError: startMutation.error,
  };
}
