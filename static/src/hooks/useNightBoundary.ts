import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { FALLBACK_NIGHT_BOUNDARY_SECONDS } from '../utils/observingNight';

/**
 * Where one catalog's nights split, in seconds after 00:00 UTC, as the server
 * counts nights for calibration and the AstroBin export. Read whatever the
 * grouping, so switching to a date grouping does not wait for it; the stats
 * share their cache with the Library's. `ready` turns true once the answer
 * is in; a catalog whose stats cannot be read uses the server's own fallback,
 * noon UTC.
 *
 * The server recomputes the boundary as frames arrive. A frame in an hour
 * that was quiet until then can move it, and with it some frames from one
 * night to the next; a link that named only nights no longer there opens
 * the default groups.
 */
export function useNightBoundary(dbId: string | null | undefined) {
  const query = useQuery({
    queryKey: ['db', dbId, 'overall-stats'],
    queryFn: () => apiClient.getOverallStats(dbId!),
    enabled: !!dbId,
    staleTime: 30_000,
  });
  return {
    boundarySeconds: query.data?.night_boundary_seconds ?? FALLBACK_NIGHT_BOUNDARY_SECONDS,
    ready: !dbId || query.isSuccess || query.isError,
  };
}
