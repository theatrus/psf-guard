import { useEffect, useRef } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { FLAT_WATCH_MS, flatMeasureKey, flatMeasurePollMs, measuringRig } from './flatMeasureLabel';

/** How often the job's progress is asked for while a pass runs, and while it
 *  rests; shared with the header's poll of the same catalog. */
export { FLAT_MEASURE_POLL_MS as FLAT_POLL_MS } from './flatMeasureLabel';

export const flatGroupsKey = (dbId: string) => ['db', dbId, 'calibrations', 'flat-groups'] as const;

/**
 * One database's flat groups, or those a project's lights touch, and
 * whether the flat measuring job is measuring the rig in view now. The job's
 * progress comes from the poll the header already makes for this catalog
 * (one request per interval however many parts of the app show it). When a
 * rig is chosen, the page also tells the server it shows that rig, which
 * puts its pass first in the viewed tier. When a pass ends, the groups are
 * read again.
 */
export function useFlatGroups(
  dbId: string,
  enabled: boolean,
  project?: number,
  revision: string | null = null,
  rig?: string,
) {
  const queryClient = useQueryClient();
  const access = useAccess();
  // Read once, and again only when a pass changed them: not on the app's
  // usual timer, since the server reads every light to answer.
  const groups = useQuery({
    // Under the database's key, so a pass's end reads every scope again.
    queryKey: [...flatGroupsKey(dbId), project ?? 'all', revision],
    queryFn: () => apiClient.getFlatGroups(dbId, project),
    enabled,
    refetchInterval: false,
  });
  const job = useQuery({
    queryKey: flatMeasureKey(dbId),
    queryFn: () => apiClient.getFlatMeasureStatus(dbId),
    enabled,
    refetchInterval: (query) => flatMeasurePollMs(query.state.data),
    refetchIntervalInBackground: false,
    retry: false,
  });
  // A hint while one rig's flats are in view; nothing when every rig is,
  // or for a viewer the server keeps from display calculations.
  useQuery({
    queryKey: [...flatMeasureKey(dbId), 'watch', rig ?? null],
    queryFn: () => apiClient.watchFlatMeasure(dbId, rig!),
    enabled: enabled && !!rig && access.canCompute,
    refetchInterval: FLAT_WATCH_MS,
    refetchIntervalInBackground: false,
    retry: false,
    staleTime: 0,
  });
  const progress = job.data;
  const measuring = measuringRig(progress, rig);

  // A pass has finished since the last poll: read the groups again.
  const lastFinished = useRef<number | null | undefined>(undefined);
  const finished = progress?.finished_at;
  const known = progress !== undefined;
  useEffect(() => {
    if (!known) return;
    const newPass = lastFinished.current !== undefined && finished !== lastFinished.current;
    lastFinished.current = finished;
    if (newPass) void queryClient.invalidateQueries({ queryKey: flatGroupsKey(dbId) });
  }, [dbId, finished, known, queryClient]);

  return { groups, progress, measuring };
}
