import { beforeEach, describe, expect, it } from 'vitest';
import {
  penaltyKeyOf,
  penaltyParamsOf,
  sameScoring,
  SCORING_DEFAULTS as DEFAULTS,
  scoringPreferences,
  setScoringPreferences,
} from '../useScoringPreferences';
import type { ScoringPreferences } from '../useScoringPreferences';

describe('scoring preferences', () => {
  beforeEach(() => {
    setScoringPreferences({ ...DEFAULTS });
  });

  it('omits defaults from the request params entirely', () => {
    expect(penaltyParamsOf(scoringPreferences())).toEqual({});
  });

  it('sends only the scales that differ from the calibrated default', () => {
    setScoringPreferences({ ...DEFAULTS, satellite: 0, temporal: 1.5 });
    expect(penaltyParamsOf(scoringPreferences())).toEqual({
      penalty_satellite: 0,
      penalty_temporal: 1.5,
    });
  });

  it('sends absolute reject limits only when set', () => {
    setScoringPreferences({
      ...DEFAULTS,
      hfrRejectAbove: 3.5,
      starCountRejectBelow: 50,
    });
    expect(penaltyParamsOf(scoringPreferences())).toEqual({
      hfr_reject_above: 3.5,
      star_count_reject_below: 50,
    });
  });

  it('derives params and key from the same snapshot', () => {
    const snapshot: ScoringPreferences = {
      ...DEFAULTS,
      satellite: 0.5,
      temporal: 2,
      hfrRejectAbove: 4,
    };
    // Pure functions of the snapshot: mutating the store afterwards must
    // not change what a caller derived from an earlier snapshot.
    const params = penaltyParamsOf(snapshot);
    const key = penaltyKeyOf(snapshot);
    setScoringPreferences({ ...DEFAULTS });
    expect(params).toEqual({
      penalty_satellite: 0.5,
      penalty_temporal: 2,
      hfr_reject_above: 4,
    });
    expect(key).toEqual([0.5, 1, 2, 4, null]);
  });

  it('clamps out-of-range and non-finite values', () => {
    setScoringPreferences({
      satellite: 9,
      pointing: -3,
      temporal: NaN,
      hfrRejectAbove: -1,
      starCountRejectBelow: NaN,
    });
    expect(scoringPreferences()).toEqual({
      satellite: 2,
      pointing: 0,
      temporal: 1,
      hfrRejectAbove: null,
      starCountRejectBelow: null,
    });
  });

  it('compares two snapshots field by field, limits included', () => {
    expect(sameScoring(DEFAULTS, { ...DEFAULTS })).toBe(true);
    expect(sameScoring(DEFAULTS, { ...DEFAULTS, hfrRejectAbove: 3.5 })).toBe(false);
    expect(sameScoring(DEFAULTS, { ...DEFAULTS, temporal: 0 })).toBe(false);
    // A cleared limit is null on both sides, never undefined vs null.
    expect(sameScoring({ ...DEFAULTS, hfrRejectAbove: null }, DEFAULTS)).toBe(true);
  });

  it('persists across a reload of the module state', () => {
    setScoringPreferences({ ...DEFAULTS, satellite: 0.5, starCountRejectBelow: 25 });
    const stored = JSON.parse(
      window.localStorage.getItem('psf-guard.penalty-scales') ?? '{}'
    );
    expect(stored.satellite).toBe(0.5);
    expect(stored.starCountRejectBelow).toBe(25);
  });
});
