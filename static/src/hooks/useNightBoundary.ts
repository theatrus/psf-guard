import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { FALLBACK_NIGHT_BOUNDARY_SECONDS } from '../utils/observingNight';

/**
 * Where one catalog's nights split, in seconds after 00:00 UTC, as the server
 * counts nights for calibration and the AstroBin export. `ready` stays false
 * until the answer is in, so a view keyed by night does not first show UTC
 * dates and then move its groups; a catalog whose stats cannot be read uses
 * the server's own fallback, noon UTC.
 */
export function useNightBoundary(dbId: string | null | undefined, enabled = true) {
  const query = useQuery({
    queryKey: ['db', dbId, 'overall-stats'],
    queryFn: () => apiClient.getOverallStats(dbId!),
    enabled: !!dbId && enabled,
    staleTime: 30_000,
  });
  return {
    boundarySeconds: query.data?.night_boundary_seconds ?? FALLBACK_NIGHT_BOUNDARY_SECONDS,
    ready: !enabled || !dbId || query.isSuccess || query.isError,
  };
}
