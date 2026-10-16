import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { parseFlatGroupFilter } from '../utils/flatGroupFilter';

/** How often what the flat groups feed is asked for while the server works it out. */
export const FLAT_GROUP_USE_POLL_MS = 3_000;

export const flatGroupUseKey = (dbId: string, group?: string) =>
  ['db', dbId, 'calibrations', 'flat-groups', 'use', group ?? 'all'] as const;

/**
 * What the flat groups feed, or one group's lights by id when `group` is
 * given. Asked again while the server works out a newer answer; it never
 * works one out inside the request.
 */
export function useFlatGroupUse(dbId: string | null, enabled: boolean, group?: string) {
  return useQuery({
    queryKey: flatGroupUseKey(dbId ?? '', group),
    queryFn: () => apiClient.getFlatGroupUse(dbId!, group),
    enabled: enabled && !!dbId,
    placeholderData: (previous) => previous,
    refetchInterval: (query) => {
      const data = query.state.data;
      return data && (data.refreshing || !data.computed) ? FLAT_GROUP_USE_POLL_MS : false;
    },
  });
}

/**
 * The Images grid's flat-group filter (`flatGroup` in the URL): the ids of
 * the lights it keeps, once known. A value made in another database keeps
 * nothing back and is not active here.
 */
export function useFlatGroupLights(dbId: string | null, value: string | null | undefined) {
  const filter = parseFlatGroupFilter(value);
  const active = !!filter && filter.dbId === dbId;
  const use = useFlatGroupUse(dbId, active, filter?.key);
  const data = use.data;
  const ids = useMemo(() => {
    const kept = new Set<number>();
    const group = data?.groups.find((entry) => entry.key === filter?.key);
    for (const project of group?.projects ?? []) {
      for (const id of (filter?.flagged ? project.flagged_ids : project.image_ids) ?? []) kept.add(id);
    }
    return kept;
  }, [data, filter?.key, filter?.flagged]);
  return {
    active,
    flagged: filter?.flagged ?? false,
    /** The lights are known: the filter may hide the rest. */
    ready: active && !!data?.computed && !use.error,
    updating: active && (!data?.computed || !!data?.refreshing),
    ids,
  };
}
