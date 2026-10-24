import { useCallback, useMemo } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useSearchParams } from 'react-router-dom';
import { apiClient } from '../api/client';
import type { PenaltyScaleParams, ScoringDefaultsSettings } from '../api/types';
import { useAccess } from '../auth/access';
import {
  clearedScoringParams,
  sameScoring,
  SCORING_DEFAULTS,
  scoringFromParams,
  scoringParamUpdates,
  scoringPreferencesOf,
  stackScoringSettings,
  type ScoringPreferences,
} from '../utils/scoring';
import { useUrlParams } from './useUrlState';

export {
  SCORING_DEFAULTS,
  penaltyKeyOf,
  sameScoring,
  type ScoringPreferences,
} from '../utils/scoring';

/*
 * How hard each kind of event evidence hits the quality score, and the
 * absolute reject limits, as every scoring surface uses them: the Sequence
 * view, the grid badges, the detail panel and stack builds.
 *
 * Two layers:
 * - The server's scoring defaults, one copy for everyone, set in
 *   Settings › Review or with the Scoring control's Save as default.
 *   Automatic stack builds score with them too, so a card they build
 *   matches what the page compares it with.
 * - The current view's own changes, from the Scoring control. They live in
 *   the link (SCORING_PARAMS), all five once any differs, so they apply to
 *   this view, travel with it to the project's other targets, Images, the
 *   detail view and Stacks (which say so, see ScoringChangedNote), stay
 *   behind when the project or database changes (keepScoringInScope), and
 *   a value the link names wins over the defaults.
 */

export const SCORING_DEFAULTS_QUERY_KEY = ['scoring-defaults'] as const;

/** One snapshot per distinct set of values, so a caller can key effects
 *  and memos on it. */
function useStableScoring(scoring: ScoringPreferences): ScoringPreferences {
  const { satellite, pointing, temporal, hfrRise, starLoss, hfrRejectAbove, starCountRejectBelow } = scoring;
  return useMemo(
    () => ({ satellite, pointing, temporal, hfrRise, starLoss, hfrRejectAbove, starCountRejectBelow }),
    [satellite, pointing, temporal, hfrRise, starLoss, hfrRejectAbove, starCountRejectBelow],
  );
}

/** The server's scoring defaults as it sends them. */
export function useScoringDefaultsQuery() {
  return useQuery({ queryKey: SCORING_DEFAULTS_QUERY_KEY, queryFn: apiClient.getScoringDefaults });
}

/** The server's scoring defaults. Until they arrive, the built-in values. */
export function useScoringDefaults(): ScoringPreferences {
  const { data } = useScoringDefaultsQuery();
  return useStableScoring(data ? scoringPreferencesOf(data.scoring) : SCORING_DEFAULTS);
}

/** Save new scoring defaults on the server. Only an editor may. */
export function useSaveScoringDefaults() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (next: ScoringPreferences) => apiClient.updateScoringDefaults(stackScoringSettings(next)),
    onSuccess: (saved: ScoringDefaultsSettings) =>
      queryClient.setQueryData(SCORING_DEFAULTS_QUERY_KEY, saved),
  });
}

/** The scoring the current view uses: what the link names, over the
 *  defaults. */
export function useScoringPreferences(): ScoringPreferences {
  const [searchParams] = useSearchParams();
  const defaults = useScoringDefaults();
  return useStableScoring(scoringFromParams(searchParams, defaults));
}

/** The view's scoring with the ways to change it, for the Scoring control. */
export function useViewScoring() {
  const { updateParams } = useUrlParams();
  const { canWrite } = useAccess();
  const scoring = useScoringPreferences();
  const defaults = useScoringDefaults();
  const saveDefaults = useSaveScoringDefaults();

  /** Score this view with `next`; the defaults stay as they are. */
  const setScoring = useCallback((next: ScoringPreferences) => {
    updateParams(scoringParamUpdates(next, defaults));
  }, [updateParams, defaults]);

  /** Make `next` the server's defaults, which this view then uses once
   *  they are saved. */
  const { mutate } = saveDefaults;
  const saveAsDefault = useCallback((next: ScoringPreferences) => {
    mutate(next, { onSuccess: () => updateParams(clearedScoringParams()) });
  }, [mutate, updateParams]);

  /** Go back to the defaults in this view. */
  const resetToDefault = useCallback(() => {
    updateParams(clearedScoringParams());
  }, [updateParams]);

  return {
    scoring,
    defaults,
    changed: !sameScoring(scoring, defaults),
    setScoring,
    /** Whether this person may change the server's defaults. */
    canSaveDefault: canWrite,
    saveAsDefault,
    savingDefault: saveDefaults.isPending,
    saveDefaultError: saveDefaults.error as Error | null,
    resetToDefault,
  };
}

/** A scoring snapshot as request query params; the calibrated values are
 * omitted so the server's built-in behavior needs no parameters at all.
 * Pure: pass the same snapshot used for the query key so key and payload
 * agree. */
export function penaltyParamsOf(preferences: ScoringPreferences): PenaltyScaleParams {
  const params: PenaltyScaleParams = {};
  if (preferences.satellite !== 1) params.penalty_satellite = preferences.satellite;
  if (preferences.pointing !== 1) params.penalty_pointing = preferences.pointing;
  if (preferences.temporal !== 1) params.penalty_temporal = preferences.temporal;
  if (preferences.hfrRise !== 1) params.penalty_hfr_rise = preferences.hfrRise;
  if (preferences.starLoss !== 1) params.penalty_star_loss = preferences.starLoss;
  if (preferences.hfrRejectAbove != null) params.hfr_reject_above = preferences.hfrRejectAbove;
  if (preferences.starCountRejectBelow != null) {
    params.star_count_reject_below = preferences.starCountRejectBelow;
  }
  return params;
}
