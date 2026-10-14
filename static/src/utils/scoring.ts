/**
 * How hard each kind of event evidence hits the quality score, and the
 * absolute reject limits. Each penalty multiplies that evidence's built-in
 * hit: 0 ignores it, 1 keeps the calibrated behavior, 2 doubles it.
 *
 * The values here are pure; where a view gets them from is in
 * hooks/useScoringPreferences.ts.
 */
export interface ScoringPreferences {
  satellite: number;
  pointing: number;
  temporal: number;
  /** Absolute HFR ceiling: recommend rejecting frames whose measured HFR
   * exceeds this. `null` turns the check off. */
  hfrRejectAbove: number | null;
  /** Absolute star-count floor: recommend rejecting frames with fewer
   * measured stars. `null` turns the check off. */
  starCountRejectBelow: number | null;
}

export type ScoringField = keyof ScoringPreferences;
export type PenaltyKey = 'satellite' | 'pointing' | 'temporal';
export type LimitKey = 'hfrRejectAbove' | 'starCountRejectBelow';

/** The calibrated, built-in values. They never change: a person's own
 * defaults are saved over them, not in place of them. */
export const SCORING_DEFAULTS: ScoringPreferences = Object.freeze({
  satellite: 1,
  pointing: 1,
  temporal: 1,
  hfrRejectAbove: null,
  starCountRejectBelow: null,
});

/*
 * What each value may be. One set of rules for the toolbar, the link,
 * Settings and stored preferences, and the same as the server's preferences
 * document checks (`KNOWN_KEYS` in src/user_preferences.rs).
 */
export const MAX_SCORING_PENALTY = 2;
/** Penalties move in tenths, as the toolbar's sliders do: 10% at a time. */
export const SCORING_PENALTY_STEP = 0.1;
export const MAX_HFR_REJECT_LIMIT = 100;
export const MAX_STAR_COUNT_REJECT_LIMIT = 1_000_000;

export interface ScoringRule {
  label: string;
  /** The smallest value. A limit must also be above zero. */
  min: number;
  max: number;
  /** Values come in whole multiples of this. */
  step?: number;
  /** A limit: null turns it off. */
  optional: boolean;
  /** How a person types and reads it: a penalty as a percentage. */
  percent: boolean;
  /** Said after the value: "percent", "pixels". */
  unit?: string;
  error: string;
}

export const SCORING_RULES: Record<ScoringField, ScoringRule> = {
  satellite: {
    label: 'Satellite trails', min: 0, max: MAX_SCORING_PENALTY, step: SCORING_PENALTY_STEP,
    optional: false, percent: true, unit: 'percent', error: 'Enter 0 to 200%, in steps of 10.',
  },
  pointing: {
    label: 'Pointing', min: 0, max: MAX_SCORING_PENALTY, step: SCORING_PENALTY_STEP,
    optional: false, percent: true, unit: 'percent', error: 'Enter 0 to 200%, in steps of 10.',
  },
  temporal: {
    label: 'Temporal anomalies', min: 0, max: MAX_SCORING_PENALTY, step: SCORING_PENALTY_STEP,
    optional: false, percent: true, unit: 'percent', error: 'Enter 0 to 200%, in steps of 10.',
  },
  hfrRejectAbove: {
    label: 'Max HFR', min: 0, max: MAX_HFR_REJECT_LIMIT, optional: true, percent: false,
    unit: 'pixels', error: `Enter a number above 0 and up to ${MAX_HFR_REJECT_LIMIT}, or leave it empty.`,
  },
  starCountRejectBelow: {
    label: 'Min stars', min: 1, max: MAX_STAR_COUNT_REJECT_LIMIT, step: 1, optional: true,
    percent: false,
    error: `Enter a whole number from 1 to ${MAX_STAR_COUNT_REJECT_LIMIT.toLocaleString('en-US')}, or leave it empty.`,
  },
};

const SCORING_KEYS = Object.keys(SCORING_RULES) as ScoringField[];

/** Within float noise of a whole multiple of `step`. */
function onStep(value: number, step: number): boolean {
  const steps = value / step;
  return Math.abs(steps - Math.round(steps)) < 1e-6;
}

/** The value `value` stands for in `field`, or undefined when it is not
 *  one the field may hold. A penalty off by float noise is put back on its
 *  step. */
export function scoringValue(field: ScoringField, value: unknown): number | null | undefined {
  const rule = SCORING_RULES[field];
  if (value === null) return rule.optional ? null : undefined;
  if (typeof value !== 'number' || !Number.isFinite(value)) return undefined;
  if (value < rule.min || value > rule.max) return undefined;
  if (rule.optional && value <= 0) return undefined;
  if (rule.step !== undefined && !onStep(value, rule.step)) return undefined;
  if (rule.step !== undefined && rule.step < 1) {
    // 0.30000000000000004 is 0.3; -0 is 0.
    return Math.round(value / rule.step) / Math.round(1 / rule.step) + 0;
  }
  return value;
}

/** Typed text as a value: the value, null for an empty limit (off), or
 *  undefined when the text is not a value the field may hold. A penalty
 *  is typed as a percentage. */
export function parseScoringText(field: ScoringField, text: string): number | null | undefined {
  const rule = SCORING_RULES[field];
  const trimmed = text.trim();
  if (trimmed === '') return rule.optional ? null : undefined;
  const number = Number(trimmed);
  if (!Number.isFinite(number)) return undefined;
  return scoringValue(field, rule.percent ? number / 100 : number);
}

/** A value as it is typed: a penalty as a percentage, a limit that is off
 *  as nothing. */
export function scoringText(field: ScoringField, value: number | null): string {
  if (value === null) return '';
  return SCORING_RULES[field].percent ? String(Math.round(value * 100)) : String(value);
}

/** Every value a field may hold, each read on its own: one it cannot hold
 *  takes `fallback`'s. */
export function normalizeScoring(
  parsed: Partial<Record<ScoringField, unknown>>,
  fallback: ScoringPreferences = SCORING_DEFAULTS,
): ScoringPreferences {
  const scoring = { ...fallback };
  for (const key of SCORING_KEYS) {
    const value = scoringValue(key, parsed[key]);
    if (value !== undefined) (scoring as Record<ScoringField, number | null>)[key] = value;
  }
  return scoring;
}

/** The same snapshot as react-query key segments. Every scoring query key
 * spreads this, so a scoring change refetches every surface. */
export function penaltyKeyOf(preferences: ScoringPreferences): (number | null)[] {
  return [
    preferences.satellite,
    preferences.pointing,
    preferences.temporal,
    preferences.hfrRejectAbove,
    preferences.starCountRejectBelow,
  ];
}

/** Whether two snapshots score identically: the one comparison every "is
 * this the default?" and "is this stale?" check shares, so a scoring knob
 * added to `penaltyKeyOf` reaches all of them at once. */
export function sameScoring(left: ScoringPreferences, right: ScoringPreferences): boolean {
  const rightKey = penaltyKeyOf(right);
  return penaltyKeyOf(left).every((value, index) => value === rightKey[index]);
}

/** A penalty as the controls show it: "off" or a percentage. */
export function formatPenalty(scale: number): string {
  return scale === 0 ? 'off' : `${Math.round(scale * 100)}%`;
}

/** A reject limit as the controls show it. */
export function formatLimit(limit: number | null): string {
  return limit === null ? 'off' : String(limit);
}

/** Any value as the controls show it. */
export function formatScoringValue(field: ScoringField, value: number | null): string {
  return SCORING_RULES[field].percent ? formatPenalty(value ?? 0) : formatLimit(value);
}

/** Where `view` departs from `defaults`, one phrase per value, such as
 *  "Satellite trails off (default 100%)". */
export function describeScoringChanges(
  view: ScoringPreferences,
  defaults: ScoringPreferences,
): string[] {
  return SCORING_KEYS
    .filter((key) => view[key] !== defaults[key])
    .map((key) => `${SCORING_RULES[key].label} ${formatScoringValue(key, view[key])} `
      + `(default ${formatScoringValue(key, defaults[key])})`);
}

/*
 * The saved defaults, as keys of the person's preferences document. They
 * sit beside the View panel's choices (see useDisplayPreferences), which is
 * how they follow a signed-in user to another browser.
 */
export const SCORING_PREFERENCE_KEYS = {
  satellite: 'scoringPenaltySatellite',
  pointing: 'scoringPenaltyPointing',
  temporal: 'scoringPenaltyTemporal',
  hfrRejectAbove: 'scoringHfrRejectAbove',
  starCountRejectBelow: 'scoringStarCountRejectBelow',
} as const satisfies Record<ScoringField, string>;

export type ScoringPreferenceFields = {
  scoringPenaltySatellite: number;
  scoringPenaltyPointing: number;
  scoringPenaltyTemporal: number;
  scoringHfrRejectAbove: number | null;
  scoringStarCountRejectBelow: number | null;
};

/** The defaults out of a preferences document; a key it lacks or cannot
 * hold reads as built in. */
export function scoringFromFields(fields: object): ScoringPreferences {
  const stored = fields as Record<string, unknown>;
  const parsed: Partial<Record<ScoringField, unknown>> = {};
  for (const key of SCORING_KEYS) parsed[key] = stored[SCORING_PREFERENCE_KEYS[key]];
  return normalizeScoring(parsed);
}

/** Defaults as preferences-document keys. */
export function fieldsFromScoring(scoring: ScoringPreferences): ScoringPreferenceFields {
  const normalized = normalizeScoring(scoring);
  return {
    scoringPenaltySatellite: normalized.satellite,
    scoringPenaltyPointing: normalized.pointing,
    scoringPenaltyTemporal: normalized.temporal,
    scoringHfrRejectAbove: normalized.hfrRejectAbove,
    scoringStarCountRejectBelow: normalized.starCountRejectBelow,
  };
}

/** Just the scoring keys a stored document carries, as stored. */
export function storedScoringFields(stored: unknown): Record<string, unknown> {
  const fields: Record<string, unknown> = {};
  if (!stored || typeof stored !== 'object') return fields;
  for (const key of Object.values(SCORING_PREFERENCE_KEYS)) {
    if (key in stored) fields[key] = (stored as Record<string, unknown>)[key];
  }
  return fields;
}

/*
 * The current view's own scoring, in the link. While the view scores as
 * the defaults do, the link names none of it, and whoever opens it scores
 * with their own defaults. Once any value differs, the link names all five,
 * so whoever opens it sees the same scores and reject recommendations.
 *
 * It belongs to the database and project it was set in: moving to another
 * drops it (see `rescope` in useUrlState), while Images, the detail view,
 * Stacks and the project's other targets keep it.
 */
export const SCORING_PARAMS = {
  satellite: 'penaltySatellite',
  pointing: 'penaltyPointing',
  temporal: 'penaltyTemporal',
  hfrRejectAbove: 'maxHfr',
  starCountRejectBelow: 'minStars',
} as const satisfies Record<ScoringField, string>;

export const SCORING_PARAM_NAMES: readonly string[] = Object.values(SCORING_PARAMS);

/** A link's value for one key, or undefined when it names none it can use. */
function paramValue(key: ScoringField, text: string | null): number | null | undefined {
  if (text === null || text.trim() === '') return undefined;
  if (text === 'off') return SCORING_RULES[key].optional ? null : undefined;
  return scoringValue(key, Number(text));
}

/** The view's scoring: each value the link names, over the defaults. */
export function scoringFromParams(
  params: { get(name: string): string | null },
  defaults: ScoringPreferences,
): ScoringPreferences {
  const scoring = { ...defaults };
  for (const key of SCORING_KEYS) {
    const value = paramValue(key, params.get(SCORING_PARAMS[key]));
    if (value !== undefined) (scoring as Record<ScoringField, number | null>)[key] = value;
  }
  return scoring;
}

/** The link updates that make a view score with `view`: nothing while it
 *  matches the defaults, and every value once any differs. */
export function scoringParamUpdates(
  view: ScoringPreferences,
  defaults: ScoringPreferences,
): Record<string, string | null> {
  const normalized = normalizeScoring(view, defaults);
  if (sameScoring(normalized, defaults)) return clearedScoringParams();
  const updates: Record<string, string | null> = {};
  for (const key of SCORING_KEYS) {
    const value = normalized[key];
    updates[SCORING_PARAMS[key]] = value === null ? 'off' : String(value);
  }
  return updates;
}

/** Updates that drop every scoring key from the link. */
export function clearedScoringParams(): Record<string, null> {
  return Object.fromEntries(SCORING_PARAM_NAMES.map((name) => [name, null]));
}
