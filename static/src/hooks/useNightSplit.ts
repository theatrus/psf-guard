import { useCallback } from 'react';
import { useQuery, type QueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useUrlParams } from './useUrlState';

/**
 * Where lights split into nights: at each rig's local noon, as calibration
 * splits its frames, or at the catalog's quiet hours, one split for every
 * rig. The `light_nights` calibration setting, kept by the server, chooses
 * for everyone; a view can show the other with `nights=` in its link, set
 * from its View panel. See docs/SCREENING.md, "Dates are nights".
 */
export type NightSplit = 'rig' | 'catalog';

export const NIGHT_SPLIT_PARAM = 'nights';

export const NIGHT_SPLIT_LABELS: Record<NightSplit, string> = {
  rig: "The rig's local noon",
  catalog: "The catalog's quiet hours",
};

export function parseNightSplit(raw: unknown): NightSplit | null {
  return raw === 'rig' || raw === 'catalog' ? raw : null;
}

/**
 * The split in effect, the server's setting, and the view's override. The
 * image list and the calibration reports send `override`, and the server
 * applies its own setting when there is none; their query keys hold
 * `queryKey`, the override or `setting`, so a cold load fetches once
 * whatever the setting, and saving the setting refreshes them
 * (`invalidateNightSplitViews`). `setting` is null until the server has
 * said; until then a choice always goes into the link.
 */
export function useNightSplit() {
  const { getParam, updateParams } = useUrlParams();
  const settings = useQuery({
    queryKey: ['calibration-settings'],
    queryFn: apiClient.getCalibrationSettings,
    staleTime: 60_000,
    retry: false,
  });
  const setting: NightSplit | null = settings.isSuccess
    ? parseNightSplit(settings.data?.rules?.light_nights) ?? 'rig'
    : null;
  const override = parseNightSplit(getParam(NIGHT_SPLIT_PARAM));

  /** Show `next` in this view; the setting's own choice clears the link. */
  const setSplit = useCallback((next: NightSplit) => {
    updateParams({ [NIGHT_SPLIT_PARAM]: next === setting ? null : next });
  }, [setting, updateParams]);

  return {
    split: override ?? setting ?? 'rig',
    setting,
    override,
    queryKey: override ?? 'setting',
    setSplit,
  };
}

/** Refresh what names lights' nights after the setting changed: every
 *  image list, image calibration account and calibration report. */
export function invalidateNightSplitViews(queryClient: QueryClient): void {
  queryClient.invalidateQueries({
    predicate: (query) => ['all-images', 'image-calibration', 'calibration-report']
      .some((part) => query.queryKey.includes(part)),
  });
}

/** What the View button says while a view overrides the setting. */
export function nightSplitNote(override: NightSplit | null, setting: NightSplit | null): string | undefined {
  if (!override || override === setting) return undefined;
  return override === 'catalog' ? 'catalog nights' : 'rig nights';
}

/**
 * `placeholderData` for a query keyed by `scope` and then the split: the
 * previous data while only the split changed, so a grid keeps its frames
 * and its place until the new nights arrive. Another scope, such as
 * another project, starts empty, as before.
 */
export function keepAcrossNightSplit<T>(scope: readonly unknown[]) {
  return (previous: T | undefined, previousQuery?: { queryKey: readonly unknown[] }): T | undefined => {
    if (!previousQuery) return undefined;
    return scope.every((part, index) => previousQuery.queryKey[index] === part) ? previous : undefined;
  };
}
