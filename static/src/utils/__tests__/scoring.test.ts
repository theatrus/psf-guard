import { describe, expect, it } from 'vitest';
import {
  clearedScoringParams,
  describeScoringChanges,
  fieldsFromScoring,
  normalizeScoring,
  parseScoringText,
  penaltyKeyOf,
  sameScoring,
  SCORING_DEFAULTS as DEFAULTS,
  scoringFromFields,
  scoringFromParams,
  scoringParamUpdates,
  scoringText,
  scoringValue,
  type ScoringPreferences,
} from '../scoring';

const params = (query: string) => new URLSearchParams(query);

describe('scoring values', () => {
  it('reads a value it may not hold as the fallback', () => {
    expect(normalizeScoring({
      satellite: 9,
      pointing: -3,
      temporal: NaN,
      hfrRejectAbove: -1,
      starCountRejectBelow: NaN,
    })).toEqual(DEFAULTS);
    // Past a cap is refused, not held to it.
    expect(normalizeScoring({ hfrRejectAbove: 250, starCountRejectBelow: 5e6 })).toEqual(DEFAULTS);
    const fallback = { ...DEFAULTS, satellite: 0.5, hfrRejectAbove: 3 };
    expect(normalizeScoring({ satellite: 0.55, hfrRejectAbove: 0 }, fallback)).toEqual(fallback);
  });

  it('holds every field to one set of rules', () => {
    // Penalties from 0 to 2 in tenths, float noise put back on its step.
    expect(scoringValue('satellite', 0.30000000000000004)).toBe(0.3);
    expect(scoringValue('pointing', -0)).toBe(0);
    expect(Object.is(scoringValue('pointing', -0), 0)).toBe(true);
    expect(scoringValue('temporal', 2)).toBe(2);
    expect(scoringValue('temporal', 0.35)).toBeUndefined();
    expect(scoringValue('temporal', 2.1)).toBeUndefined();
    expect(scoringValue('satellite', null)).toBeUndefined();
    // Max HFR above zero up to 100, any decimal, or off.
    expect(scoringValue('hfrRejectAbove', 2.55)).toBe(2.55);
    expect(scoringValue('hfrRejectAbove', 100)).toBe(100);
    expect(scoringValue('hfrRejectAbove', null)).toBeNull();
    expect(scoringValue('hfrRejectAbove', 0)).toBeUndefined();
    expect(scoringValue('hfrRejectAbove', 150)).toBeUndefined();
    // Min stars a whole count from 1 to a million, or off.
    expect(scoringValue('starCountRejectBelow', 505)).toBe(505);
    expect(scoringValue('starCountRejectBelow', 1_000_000)).toBe(1_000_000);
    expect(scoringValue('starCountRejectBelow', 12.5)).toBeUndefined();
    expect(scoringValue('starCountRejectBelow', 0)).toBeUndefined();
    expect(scoringValue('starCountRejectBelow', '505')).toBeUndefined();
  });

  it('reads typed text by the same rules, a penalty as a percentage', () => {
    expect(parseScoringText('satellite', '30')).toBe(0.3);
    expect(parseScoringText('satellite', '35')).toBeUndefined();
    expect(parseScoringText('satellite', '')).toBeUndefined();
    expect(parseScoringText('hfrRejectAbove', ' ')).toBeNull();
    expect(parseScoringText('hfrRejectAbove', '0.8')).toBe(0.8);
    expect(parseScoringText('hfrRejectAbove', '150')).toBeUndefined();
    expect(parseScoringText('starCountRejectBelow', '12.5')).toBeUndefined();
    expect(parseScoringText('starCountRejectBelow', 'abc')).toBeUndefined();
    expect(scoringText('pointing', 1.1)).toBe('110');
    expect(scoringText('starCountRejectBelow', null)).toBe('');
  });

  it('names what a view changes', () => {
    expect(describeScoringChanges(DEFAULTS, DEFAULTS)).toEqual([]);
    expect(describeScoringChanges(
      { ...DEFAULTS, satellite: 0, starCountRejectBelow: 135 },
      { ...DEFAULTS, hfrRejectAbove: 3, starCountRejectBelow: 125 },
    )).toEqual([
      'Satellite trails off (default 100%)',
      'Max HFR off (default 3)',
      'Min stars 135 (default 125)',
    ]);
  });

  it('compares two snapshots field by field, limits included', () => {
    expect(sameScoring(DEFAULTS, { ...DEFAULTS })).toBe(true);
    expect(sameScoring(DEFAULTS, { ...DEFAULTS, hfrRejectAbove: 3.5 })).toBe(false);
    expect(sameScoring(DEFAULTS, { ...DEFAULTS, temporal: 0 })).toBe(false);
    expect(sameScoring(DEFAULTS, { ...DEFAULTS, hfrRise: 2 })).toBe(false);
    expect(sameScoring(DEFAULTS, { ...DEFAULTS, starLoss: 0.5 })).toBe(false);
    expect(sameScoring({ ...DEFAULTS, hfrRejectAbove: null }, DEFAULTS)).toBe(true);
    expect(penaltyKeyOf({ ...DEFAULTS, satellite: 0.5, hfrRejectAbove: 4 }))
      .toEqual([0.5, 1, 1, 1, 1, 4, null]);
  });

  it('keeps the built-in values as they were', () => {
    expect(DEFAULTS).toEqual({
      satellite: 1,
      pointing: 1,
      temporal: 1,
      hfrRise: 1,
      starLoss: 1,
      hfrRejectAbove: null,
      starCountRejectBelow: null,
    });
  });
});

describe('scoring defaults in the preferences document', () => {
  it('round-trips through the document keys', () => {
    const scoring: ScoringPreferences = {
      satellite: 0,
      pointing: 1.5,
      temporal: 1,
      hfrRise: 2,
      starLoss: 0.5,
      hfrRejectAbove: 2.55,
      starCountRejectBelow: null,
    };
    const fields = fieldsFromScoring(scoring);
    expect(fields).toEqual({
      scoringPenaltySatellite: 0,
      scoringPenaltyPointing: 1.5,
      scoringPenaltyTemporal: 1,
      scoringPenaltyHfrRise: 2,
      scoringPenaltyStarLoss: 0.5,
      scoringHfrRejectAbove: 2.55,
      scoringStarCountRejectBelow: null,
    });
    expect(scoringFromFields(fields)).toEqual(scoring);
  });

  it('reads a document without them as built in', () => {
    expect(scoringFromFields({ imageOrder: 'newest' })).toEqual(DEFAULTS);
    expect(scoringFromFields({ scoringPenaltySatellite: 'lots', scoringHfrRejectAbove: 0 }))
      .toEqual(DEFAULTS);
  });
});

describe('a view\'s scoring in the link', () => {
  const defaults: ScoringPreferences = { ...DEFAULTS, satellite: 0.5, hfrRejectAbove: 3 };

  it('uses the defaults where the link names nothing', () => {
    expect(scoringFromParams(params('db=a&project=1'), defaults)).toEqual(defaults);
  });

  it('lets a value the link names win', () => {
    expect(scoringFromParams(params('penaltySatellite=0&maxHfr=off&minStars=505'), defaults))
      .toEqual({ ...defaults, satellite: 0, hfrRejectAbove: null, starCountRejectBelow: 505 });
    expect(scoringFromParams(params('penaltyPointing=2&penaltyTemporal=0.3'), defaults))
      .toEqual({ ...defaults, pointing: 2, temporal: 0.3 });
  });

  it('ignores a value it may not hold, by the rules Settings uses', () => {
    expect(scoringFromParams(params('penaltySatellite=lots&maxHfr=-1&minStars='), defaults))
      .toEqual(defaults);
    expect(scoringFromParams(params('penaltySatellite=7&maxHfr=500&minStars=12.5'), defaults))
      .toEqual(defaults);
    expect(scoringFromParams(params('penaltySatellite=0.35&penaltyPointing=off&maxHfr=OFF'), defaults))
      .toEqual(defaults);
  });

  it('writes nothing at the defaults, and every value once one differs', () => {
    expect(scoringParamUpdates(defaults, defaults)).toEqual(clearedScoringParams());
    // So whoever opens the link scores the same way, whatever their defaults.
    expect(scoringParamUpdates({ ...defaults, starCountRejectBelow: 505 }, defaults)).toEqual({
      penaltySatellite: '0.5',
      penaltyPointing: '1',
      penaltyTemporal: '1',
      penaltyHfrRise: '1',
      penaltyStarLoss: '1',
      maxHfr: '3',
      minStars: '505',
    });
    expect(scoringParamUpdates({ ...defaults, hfrRejectAbove: null }, defaults)).toMatchObject({
      maxHfr: 'off',
      minStars: 'off',
    });
  });

  it('reproduces the view for someone with other defaults', () => {
    const view: ScoringPreferences = { ...defaults, temporal: 0 };
    const query = new URLSearchParams();
    for (const [key, value] of Object.entries(scoringParamUpdates(view, defaults))) {
      if (value !== null) query.set(key, value);
    }
    const theirs: ScoringPreferences = { ...DEFAULTS, pointing: 2, starCountRejectBelow: 900 };
    expect(scoringFromParams(query, theirs)).toEqual(view);
  });

  it('reads back what it writes', () => {
    const view: ScoringPreferences = {
      satellite: 0, pointing: 1.2, temporal: 1, hfrRise: 1.5, starLoss: 0, hfrRejectAbove: null,
      starCountRejectBelow: 40,
    };
    const updates = scoringParamUpdates(view, defaults);
    const query = new URLSearchParams();
    for (const [key, value] of Object.entries(updates)) if (value !== null) query.set(key, value);
    expect(scoringFromParams(query, defaults)).toEqual(view);
  });
});
