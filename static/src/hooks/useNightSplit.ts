import { useCallback } from 'react';
import { useQuery } from '@tanstack/react-query';
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
 * image list and the calibration reports send `override` so the server
 * names nights the same way; the effective `split` belongs in their query
 * keys, so a change to either refetches them.
 */
export function useNightSplit() {
  const { getParam, updateParams } = useUrlParams();
  const settings = useQuery({
    queryKey: ['calibration-settings'],
    queryFn: apiClient.getCalibrationSettings,
    staleTime: 60_000,
    retry: false,
  });
  const setting = parseNightSplit(settings.data?.rules?.light_nights) ?? 'rig';
  const override = parseNightSplit(getParam(NIGHT_SPLIT_PARAM));

  /** Show `next` in this view; the setting's own choice clears the link. */
  const setSplit = useCallback((next: NightSplit) => {
    updateParams({ [NIGHT_SPLIT_PARAM]: next === setting ? null : next });
  }, [setting, updateParams]);

  return { split: override ?? setting, setting, override, setSplit };
}

/** What the View button says while a view overrides the setting. */
export function nightSplitNote(override: NightSplit | null, setting: NightSplit): string | undefined {
  if (!override || override === setting) return undefined;
  return override === 'catalog' ? 'catalog nights' : 'rig nights';
}
