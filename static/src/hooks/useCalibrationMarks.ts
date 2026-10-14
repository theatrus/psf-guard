import { useMemo } from 'react';
import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { CalibrationLightMark } from '../api/types';

const EMPTY = new Map<number, CalibrationLightMark>();
const NO_PROJECTS = new Set<number>();

/**
 * Which lights the calibration library serves poorly, by image id, from
 * the background health results: for one project, or the whole database
 * when none is in scope. Reads only while `enabled`, and again every few
 * seconds while a pass works results out.
 */
export function useCalibrationMarks(dbId: string | null, projectId: number | null, enabled: boolean) {
  const query = useQuery({
    queryKey: ['db', dbId, 'calibration-marks', projectId ?? 'all'],
    queryFn: () => apiClient.getCalibrationLightMarks(dbId!, projectId),
    enabled: enabled && !!dbId,
    staleTime: 30_000,
    placeholderData: keepPreviousData,
    refetchInterval: (current) => {
      const data = current.state.data;
      if (!data) return false;
      if (data.running) return 5_000;
      return data.stale_projects.length > 0 ? 15_000 : false;
    },
  });
  const marks = useMemo(
    () => (query.data ? new Map(query.data.lights.map((mark) => [mark.image_id, mark])) : EMPTY),
    [query.data],
  );
  const staleProjects = useMemo(
    () => (query.data ? new Set(query.data.stale_projects) : NO_PROJECTS),
    [query.data],
  );
  return {
    marks,
    /** Projects whose marks came before a change, until a pass replaces
     *  them. Fine to show; not to act on. */
    staleProjects,
    isLoading: enabled && query.isLoading,
    error: query.error,
    /** A pass is working results out, or one is due. */
    updating: !!query.data && (query.data.running || query.data.stale_projects.length > 0),
  };
}
