import { beforeEach, describe, expect, it } from 'vitest';
import {
  scoringPenaltyParams,
  scoringPreferences,
  setScoringPreferences,
} from '../useScoringPreferences';

describe('scoring preferences', () => {
  beforeEach(() => {
    setScoringPreferences({ satellite: 1, pointing: 1, temporal: 1 });
  });

  it('omits defaults from the request params entirely', () => {
    expect(scoringPenaltyParams()).toEqual({});
  });

  it('sends only the scales that differ from the calibrated default', () => {
    setScoringPreferences({ satellite: 0, pointing: 1, temporal: 1.5 });
    expect(scoringPenaltyParams()).toEqual({
      penalty_satellite: 0,
      penalty_temporal: 1.5,
    });
  });

  it('clamps out-of-range and non-finite values', () => {
    setScoringPreferences({ satellite: 9, pointing: -3, temporal: NaN });
    expect(scoringPreferences()).toEqual({ satellite: 2, pointing: 0, temporal: 1 });
  });

  it('persists across a reload of the module state', () => {
    setScoringPreferences({ satellite: 0.5, pointing: 1, temporal: 1 });
    const stored = JSON.parse(
      window.localStorage.getItem('psf-guard.penalty-scales') ?? '{}'
    );
    expect(stored.satellite).toBe(0.5);
  });
});
