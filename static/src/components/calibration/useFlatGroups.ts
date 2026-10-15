import { useEffect, useRef } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';

/** The quality job's stages that work on flat groups. */
const FLAT_STAGES = new Set([
  'measuring flat sessions',
  'comparing flat sessions',
  'reading lights shot during a change',
]);

/** How often the job's progress is asked for while it runs, and while it rests. */
export const FLAT_POLL_MS = { running: 3_000, idle: 30_000 };

export const flatGroupsKey = (dbId: string) => ['db', dbId, 'calibrations', 'flat-groups'] as const;

/**
 * One database's flat groups, or those a project's lights touch, and whether the background pass is measuring
 * them now. The page holds one of these, so one poll asks for the job's
 * progress however many parts of the page show it; when a pass that worked
 * on flats ends, the groups are read again.
 */
export function useFlatGroups(dbId: string, enabled: boolean, project?: number, revision: string | null = null) {
  const queryClient = useQueryClient();
  // Read once, and again only when a pass changed them: not on the app's
  // usual timer, since the server reads every light to answer.
  const groups = useQuery({
    // Under the database's key, so a pass's end reads every scope again.
    queryKey: [...flatGroupsKey(dbId), project ?? 'all', revision],
    queryFn: () => apiClient.getFlatGroups(dbId, project),
    enabled,
    refetchInterval: false,
  });
  const quality = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'quality'],
    queryFn: () => apiClient.getCalibrationQuality(dbId),
    enabled,
    refetchInterval: (query) => (query.state.data?.progress.running ? FLAT_POLL_MS.running : FLAT_POLL_MS.idle),
    retry: false,
  });
  const progress = quality.data?.progress;
  const measuring = !!progress?.running && FLAT_STAGES.has(progress.stage ?? '');

  // A pass that worked on flats has finished, or another pass finished
  // since the last poll: read the groups again.
  const wasMeasuring = useRef(false);
  const lastFinished = useRef<number | null | undefined>(undefined);
  const finished = progress?.finished_at;
  const known = progress !== undefined;
  useEffect(() => {
    const ended = wasMeasuring.current && !measuring;
    const newPass = lastFinished.current !== undefined && finished !== lastFinished.current;
    wasMeasuring.current = measuring;
    if (known) lastFinished.current = finished;
    if (ended || newPass) void queryClient.invalidateQueries({ queryKey: flatGroupsKey(dbId) });
  }, [dbId, finished, known, measuring, queryClient]);

  return { groups, progress, measuring };
}
