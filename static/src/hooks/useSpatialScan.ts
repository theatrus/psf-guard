import { useEffect, useRef } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { SpatialScanStatus } from '../api/types';

/**
 * Trigger and monitor the server-side spatial (occlusion) metrics scan.
 *
 * The scan reads every FITS file of the target and runs star detection, so it
 * takes seconds per frame; the server runs it in the background and this hook
 * polls progress at 1s while it runs (same pattern as cache refresh). When a
 * scan finishes, all sequence-analysis queries for the DB are invalidated so
 * the view re-fetches with the merged occlusion metrics.
 */
export function useSpatialScan(
  dbId: string | null | undefined,
  targetId: number | undefined,
  filterName?: string
) {
  const queryClient = useQueryClient();

  const statusQuery = useQuery<SpatialScanStatus>({
    queryKey: ['db', dbId, 'spatial-scan'],
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

  const startMutation = useMutation({
    mutationFn: (force?: boolean) =>
      apiClient.startSpatialScan(dbId!, {
        target_id: targetId!,
        filter_name: filterName,
        force,
      }),
    onSuccess: (status) => {
      // Seed the poll query so refetchInterval kicks in immediately.
      queryClient.setQueryData(['db', dbId, 'spatial-scan'], status);
      if (!status.started && !status.progress.running) {
        // Nothing needed computing; metrics may still be newly relevant.
        queryClient.invalidateQueries({ queryKey: ['db', dbId, 'sequence-analysis'] });
      }
    },
  });

  return {
    status: statusQuery.data,
    isRunning,
    start: startMutation.mutate,
    isStarting: startMutation.isPending,
    startError: startMutation.error,
  };
}
