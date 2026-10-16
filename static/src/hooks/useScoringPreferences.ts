import { useCallback, useMemo } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { PenaltyScaleParams } from '../api/types';
import {
  clearedScoringParams,
  fieldsFromScoring,
  sameScoring,
  scoringFromFields,
  scoringFromParams,
  scoringParamUpdates,
  type ScoringPreferences,
} from '../utils/scoring';
import {
  getDisplayPreferences,
  setDisplayPreferences,
  useDisplayPreferences,
} from './useDisplayPreferences';
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
 * - The person's defaults, set in Settings › Review or with the Scoring
 *   control's Save as default. They live with the View panel's choices
 *   (useDisplayPreferences), so a signed-in user's follow them to another
 *   browser.
 * - The current view's own changes, from the Scoring control. They live in
 *   the link (SCORING_PARAMS), all five once any differs, so they apply to
 *   this view, travel with it to the project's other targets, Images, the
 *   detail view and Stacks (which say so, see ScoringChangedNote), stay
 *   behind when the project or database changes (keepScoringInScope), and
 *   a value the link names wins over the defaults.
 */

/** The saved defaults now, outside React. */
export function scoringDefaults(): ScoringPreferences {
  return scoringFromFields(getDisplayPreferences());
}

/** Save new defaults. */
export function setScoringDefaults(next: ScoringPreferences): void {
  setDisplayPreferences({ ...getDisplayPreferences(), ...fieldsFromScoring(next) });
}

/** One snapshot per distinct set of values, so a caller can key effects
 *  and memos on it. */
function useStableScoring(scoring: ScoringPreferences): ScoringPreferences {
  const { satellite, pointing, temporal, hfrRise, starLoss, hfrRejectAbove, starCountRejectBelow } = scoring;
  return useMemo(
    () => ({ satellite, pointing, temporal, hfrRise, starLoss, hfrRejectAbove, starCountRejectBelow }),
    [satellite, pointing, temporal, hfrRise, starLoss, hfrRejectAbove, starCountRejectBelow],
  );
}

/** The saved defaults. */
export function useScoringDefaults(): ScoringPreferences {
  const preferences = useDisplayPreferences();
  return useStableScoring(scoringFromFields(preferences));
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
  const scoring = useScoringPreferences();
  const defaults = useScoringDefaults();

  /** Score this view with `next`; the defaults stay as they are. */
  const setScoring = useCallback((next: ScoringPreferences) => {
    updateParams(scoringParamUpdates(next, scoringDefaults()));
  }, [updateParams]);

  /** Make `next` the defaults, which this view then uses. */
  const saveAsDefault = useCallback((next: ScoringPreferences) => {
    setScoringDefaults(next);
    updateParams(clearedScoringParams());
  }, [updateParams]);

  /** Go back to the defaults in this view. */
  const resetToDefault = useCallback(() => {
    updateParams(clearedScoringParams());
  }, [updateParams]);

  return {
    scoring,
    defaults,
    changed: !sameScoring(scoring, defaults),
    setScoring,
    saveAsDefault,
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
