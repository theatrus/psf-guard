import type {
  CalibrationCheckMode,
  CalibrationGradeLimits,
  CalibrationRuleField,
  CalibrationRuleValue,
  CalibrationRuleValues,
} from '../api/types';

/**
 * The calibration checks a person can tune, as the settings panel edits
 * them. The server sends every setting's range and default
 * (`CalibrationSettings.fields`) and checks every value again on save; these
 * helpers turn a value into what a field shows and a field's text back into
 * a value.
 */

/** What one field holds while it is edited: text for numbers, the value for the rest. */
export type RuleDraft = string | boolean | { suspect: string; poor: string };

export const RULE_GROUPS: ReadonlyArray<{ group: CalibrationRuleField['group']; legend: string }> = [
  { group: 'matching', legend: 'Matching' },
  { group: 'darks', legend: 'Dark masters' },
  { group: 'flat_groups', legend: 'Flat groups' },
  { group: 'checks', legend: 'Light checks' },
  { group: 'grades', legend: 'Frame grades' },
];

function trimNumber(value: number): string {
  return String(Math.round(value * 10_000) / 10_000);
}

/** A unit as words, for a sentence: `°` is degrees, `h` hours. */
export function unitWords(unit: string): string {
  if (unit === '°') return 'degrees';
  if (unit === 'h') return 'hours';
  return unit;
}

function isLimits(value: unknown): value is CalibrationGradeLimits {
  return typeof value === 'object' && value !== null && 'suspect' in value && 'poor' in value;
}

/** Whether two values mean the same: `2` and `2.0` are one tolerance. */
export function sameRuleValue(left: CalibrationRuleValue | undefined, right: CalibrationRuleValue | undefined): boolean {
  if (isLimits(left) && isLimits(right)) return left.suspect === right.suspect && left.poor === right.poor;
  return left === right;
}

/** Whether two sets of values hold the same keys and values, whatever order they came in. */
export function sameRules(left: CalibrationRuleValues, right: CalibrationRuleValues): boolean {
  const keys = Object.keys(left);
  return (
    keys.length === Object.keys(right).length &&
    keys.every((key) => key in right && sameRuleValue(left[key], right[key]))
  );
}

/** A key for a set of values that changes only when the values do, not their order. */
export function rulesKey(rules: CalibrationRuleValues): string {
  return JSON.stringify(
    Object.keys(rules)
      .sort()
      .map((key) => {
        const value = rules[key];
        return [key, isLimits(value) ? [value.suspect ?? null, value.poor ?? null] : value];
      }),
  );
}

/** The labels a check's modes show: what the check then raises. */
export function checkOptions(field: CalibrationRuleField): { value: CalibrationCheckMode; label: string }[] {
  if (field.kind.type !== 'check') return [];
  const poor = field.kind.levels.includes('poor');
  return poor
    ? [
        { value: 'flag', label: field.kind.levels.includes('check') ? 'Flag' : 'Poor' },
        { value: 'advisory', label: 'Check only' },
        { value: 'off', label: 'Off' },
      ]
    : [
        { value: 'flag', label: 'Check' },
        { value: 'off', label: 'Off' },
      ];
}

/** A value in words, as the light's account and the inherited labels name it. */
export function describeRuleValue(field: CalibrationRuleField, value: CalibrationRuleValue): string {
  const kind = field.kind;
  switch (kind.type) {
    case 'number': {
      if (typeof value !== 'number') return String(value);
      if (value === 0 && kind.zero) return kind.zero;
      return kind.unit === '°' ? `${trimNumber(value)}°` : `${trimNumber(value)} ${kind.unit}`;
    }
    case 'count':
      return `${value} ${kind.unit}`;
    case 'toggle':
      return value === true ? 'on' : 'off';
    case 'choice':
      return kind.options.find((option) => option.value === value)?.label ?? String(value);
    case 'check':
      return checkOptions(field).find((option) => option.value === value)?.label.toLowerCase() ?? String(value);
    case 'limits': {
      if (!isLimits(value)) return String(value);
      const step = (limit: number | null) => {
        if (limit === null) return 'never';
        if (kind.percent) return `${trimNumber(limit * 100)}%`;
        return kind.unit === '×' ? `${trimNumber(limit)}×` : `${trimNumber(limit)} ${kind.unit}`;
      };
      return `suspect ${step(value.suspect)}, poor ${step(value.poor)}`;
    }
  }
}

/** What a field shows for a value. */
export function draftOf(field: CalibrationRuleField, value: CalibrationRuleValue | undefined): RuleDraft {
  const kind = field.kind;
  switch (kind.type) {
    case 'number':
    case 'count':
      return typeof value === 'number' ? trimNumber(value) : '';
    case 'toggle':
      return typeof value === 'boolean' ? value : field.default === true;
    case 'choice':
    case 'check':
      return typeof value === 'string' ? value : String(field.default);
    case 'limits': {
      const limits = isLimits(value) ? value : (field.default as CalibrationGradeLimits);
      const step = (limit: number | null) =>
        limit === null ? '' : trimNumber(kind.percent ? limit * 100 : limit);
      return { suspect: step(limits.suspect), poor: step(limits.poor) };
    }
  }
}

export type ParsedDraft =
  | { ok: true; value: CalibrationRuleValue | null }
  | { ok: false; error: string };

/**
 * A field's text as a value. An empty number field is `null`: the default,
 * or the inherited value. An empty grade limit is `null` too: never.
 */
export function parseDraft(field: CalibrationRuleField, draft: RuleDraft): ParsedDraft {
  const kind = field.kind;
  switch (kind.type) {
    case 'number':
    case 'count': {
      const text = typeof draft === 'string' ? draft.trim() : '';
      if (text === '') return { ok: true, value: null };
      const value = Number(text);
      const whole = kind.type === 'count';
      if (!Number.isFinite(value) || value < kind.min || value > kind.max || (whole && !Number.isInteger(value))) {
        return { ok: false, error: `Enter ${kind.min} to ${kind.max} ${unitWords(kind.unit)}.` };
      }
      return { ok: true, value };
    }
    case 'toggle':
      return { ok: true, value: draft === true };
    case 'choice':
    case 'check':
      return { ok: true, value: String(draft) };
    case 'limits': {
      if (typeof draft !== 'object') return { ok: false, error: 'Enter the limits.' };
      const step = (text: string): number | null | undefined => {
        if (text.trim() === '') return null;
        const shown = Number(text);
        if (!Number.isFinite(shown)) return undefined;
        const value = kind.percent ? shown / 100 : shown;
        return value < kind.min || value > kind.max ? undefined : Math.round(value * 1e9) / 1e9;
      };
      const suspect = step(draft.suspect);
      const poor = step(draft.poor);
      const scale = kind.percent ? 100 : 1;
      if (suspect === undefined || poor === undefined) {
        return {
          ok: false,
          error: `Enter ${kind.min * scale} to ${kind.max * scale}${kind.percent ? '%' : ''}, or leave a limit empty for never.`,
        };
      }
      if (suspect !== null && poor !== null && (kind.lower_is_worse ? suspect < poor : suspect > poor)) {
        return { ok: false, error: 'A frame must turn suspect before it turns poor.' };
      }
      return { ok: true, value: { suspect, poor } };
    }
  }
}

/** A named starting point for a rig: the values it sets. Applying one only fills the form. */
export interface RigPreset {
  id: string;
  label: string;
  hint: string;
  rules: CalibrationRuleValues;
}

export const RIG_PRESETS: ReadonlyArray<RigPreset> = [
  {
    id: 'sky-flats',
    label: 'Sky flats',
    hint:
      'Flats shot on the twilight sky: their light pattern and level change as the sky does, and a master may pool a few evenings.',
    rules: {
      flat_session_hours: 72,
      check_mixed_flats: 'off',
      grade_flat_illumination: { suspect: 0.05, poor: 0.1 },
      grade_flat_dim: { suspect: 0.05, poor: 0.02 },
    },
  },
  {
    id: 'dark-library',
    label: 'Dark library',
    hint:
      'A rig that cannot cap for darks and uses a library shot elsewhere: older darks still match, and a missing bias or dark is advisory.',
    rules: {
      dark_reach_days: 730,
      check_no_bias_or_dark: 'advisory',
    },
  },
  {
    id: 'no-darks',
    label: 'No darks',
    hint: 'Bias, dark flats and flats only: darks are left out, and the light is not flagged for lacking one.',
    rules: {
      use_darks: false,
    },
  },
];

/**
 * Bring one setting's row into view and focus its first control, as a link
 * from elsewhere asks. False when the row is not drawn.
 */
export function focusRuleRow(container: ParentNode | null | undefined, key: string): boolean {
  const row = container?.querySelector<HTMLElement>(`[data-rule='${key}']`);
  if (!row) return false;
  row.scrollIntoView?.({ block: 'center' });
  row
    .querySelector<HTMLElement>('input:not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled])')
    ?.focus({ preventScroll: true });
  return true;
}
