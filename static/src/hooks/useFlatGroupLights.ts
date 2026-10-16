import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { parseFlatGroupFilter } from '../utils/flatGroupFilter';

/** How often what the flat groups feed is asked for while the server works it out. */
export const FLAT_GROUP_USE_POLL_MS = 3_000;
/** The longest wait between asks after failures. */
const FLAT_GROUP_USE_MAX_POLL_MS = 60_000;

export const flatGroupUseKey = (dbId: string, group?: string) =>
  ['db', dbId, 'calibrations', 'flat-groups', 'use', group ?? 'all'] as const;

/** A wait that doubles with each failure in a row, up to a minute. */
function backoff(failures: number): number {
  return Math.min(FLAT_GROUP_USE_MAX_POLL_MS, FLAT_GROUP_USE_POLL_MS * 2 ** Math.max(0, failures));
}

/**
 * What the flat groups feed, or one group's lights by id when `group` is
 * given. Asked again while the server works out a newer answer; after a
 * failure, of the request or of the server's work, asked again after a
 * growing wait. The server never works one out inside the request.
 */
export function useFlatGroupUse(dbId: string | null, enabled: boolean, group?: string) {
  return useQuery({
    queryKey: flatGroupUseKey(dbId ?? '', group),
    queryFn: () => apiClient.getFlatGroupUse(dbId!, group),
    enabled: enabled && !!dbId,
    placeholderData: (previous) => previous,
    retry: 2,
    retryDelay: (attempt) => backoff(attempt),
    refetchInterval: (query) => {
      const data = query.state.data;
      if (query.state.status === 'error') return backoff(query.state.errorUpdateCount);
      if (data?.error) return FLAT_GROUP_USE_MAX_POLL_MS / 4;
      return data && (data.refreshing || !data.computed) ? FLAT_GROUP_USE_POLL_MS : false;
    },
  });
}

/** What the Images grid's flat-group filter knows. */
export type FlatGroupLightsState =
  /** No filter, or one made in another database. */
  | 'inactive'
  /** The lights are being found. */
  | 'finding'
  /** The lights are known. */
  | 'ready'
  /** The key names no group the flat groups hold now. */
  | 'missing'
  /** They could not be found. */
  | 'failed';

/**
 * The Images grid's flat-group filter (`flatGroup` in the URL): the ids of
 * the lights it keeps, once known. A value made in another database is not
 * active here.
 */
export function useFlatGroupLights(dbId: string | null, value: string | null | undefined) {
  const filter = parseFlatGroupFilter(value);
  const active = !!filter && filter.dbId === dbId;
  const use = useFlatGroupUse(dbId, active, filter?.key);
  const data = use.data;
  const key = data?.group_key ?? filter?.key;
  const ids = useMemo(() => {
    const kept = new Set<number>();
    const group = data?.groups.find((entry) => entry.key === key);
    for (const project of group?.projects ?? []) {
      for (const id of (filter?.flagged ? project.flagged_ids : project.image_ids) ?? []) kept.add(id);
    }
    return kept;
  }, [data, key, filter?.flagged]);
  const failure = use.error ?? use.failureReason;
  const state: FlatGroupLightsState = !active ? 'inactive'
    : data?.computed && data.group_found === false ? 'missing'
      : data?.computed ? 'ready'
        : failure || data?.error ? 'failed'
          : 'finding';
  return {
    active,
    flagged: filter?.flagged ?? false,
    state,
    /** The lights are known: the filter keeps only them. */
    ready: state === 'ready',
    /** A newer answer is on its way; the lights shown are the last known. */
    updating: state === 'ready' && !!data?.refreshing,
    /** Why the lights could not be found, while they could not. */
    error: state === 'failed' ? (failure instanceof Error ? failure.message : data?.error ?? 'unknown error') : null,
    ids,
  };
}
