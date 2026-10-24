import { useId, useState, type FormEvent } from 'react';
import { useAccess } from '../auth/access';
import { useSaveScoringDefaults, useScoringDefaults } from '../hooks/useScoringPreferences';
import {
  formatScoringValue,
  parseScoringText,
  SCORING_DEFAULTS,
  SCORING_RULES,
  sameScoring,
  scoringPreferencesOf,
  scoringText,
  type ScoringField,
  type ScoringPreferences,
} from '../utils/scoring';
import UnsavedMark from './UnsavedMark';

type Texts = Record<ScoringField, string>;

const HINTS: Record<ScoringField, string> = {
  satellite: 'The score penalty for a pixel-confirmed trail. At 0% trails neither lower the score nor drive reject recommendations.',
  pointing: 'The score penalty for off-target, pointing-jump and drift evidence, and the solve failures it corroborates.',
  temporal: 'The score penalty when a frame suddenly departs from its neighbours.',
  hfrRise: 'Penalty for HFR above the sequence\'s sharpest frames. At 200%, half the HFR increase produces the same penalty. At 0%, this penalty is disabled. Sudden jumps count under Temporal anomalies.',
  starLoss: 'Penalty for fewer stars than the sequence\'s best frames. At 200%, half the star-count decrease produces the same penalty. At 0%, this penalty is disabled. Sudden drops count under Temporal anomalies.',
  hfrRejectAbove: 'Recommend rejecting a frame whose measured HFR is over this. Leave blank to disable this limit.',
  starCountRejectBelow: 'Recommend rejecting a frame with fewer detected stars than this. Leave blank to disable this limit.',
};

/** The unit shown beside a field; its name is part of the field's. */
const UNIT_MARKS: Partial<Record<ScoringField, string>> = {
  satellite: '%',
  pointing: '%',
  temporal: '%',
  hfrRise: '%',
  starLoss: '%',
  hfrRejectAbove: 'px',
};

const PENALTIES: ScoringField[] = ['satellite', 'pointing', 'temporal', 'hfrRise', 'starLoss'];
const LIMITS: ScoringField[] = ['hfrRejectAbove', 'starCountRejectBelow'];
const FIELDS: ScoringField[] = [...PENALTIES, ...LIMITS];

function textsOf(scoring: ScoringPreferences): Texts {
  const texts = {} as Texts;
  for (const key of FIELDS) texts[key] = scoringText(key, scoring[key]);
  return texts;
}

function sameTexts(left: Texts, right: Texts): boolean {
  return FIELDS.every((key) => left[key] === right[key]);
}

/**
 * Settings › Review › Sequence scoring: the server's defaults for the
 * Scoring control. Every view starts from them, for everyone on the server,
 * and automatic stack builds score with them; a change in a view's toolbar
 * stays in that view until it is saved as the default. Only an editor may
 * change them. Each field follows the same rules as the toolbar and the
 * link (SCORING_RULES), so a default saved anywhere opens here as a value
 * the form accepts.
 */
export default function ScoringDefaultsSettings() {
  const headingId = useId();
  const fieldId = useId();
  const defaults = useScoringDefaults();
  const saveDefaults = useSaveScoringDefaults();
  const { canWrite } = useAccess();
  // The defaults the fields started from. Newer ones (the toolbar's Save as
  // default, another tab, a sign-in) replace untouched fields, but never
  // what the person is typing: they are offered instead.
  const [base, setBase] = useState(defaults);
  const [texts, setTexts] = useState<Texts>(() => textsOf(defaults));
  // Text a number field cannot read, such as "1e", which the browser
  // reports as empty: it must not save as "off".
  const [unreadable, setUnreadable] = useState<Partial<Record<ScoringField, boolean>>>({});
  const edited = !sameTexts(texts, textsOf(base)) || Object.values(unreadable).some(Boolean);
  if (!sameScoring(base, defaults) && !edited) {
    setBase(defaults);
    setTexts(textsOf(defaults));
  }
  const newerArrived = !sameScoring(base, defaults) && edited;

  const parsed = {} as Record<ScoringField, number | null | undefined>;
  for (const key of FIELDS) {
    parsed[key] = unreadable[key] ? undefined : parseScoringText(key, texts[key]);
  }
  const valid = FIELDS.every((key) => parsed[key] !== undefined);
  const values = parsed as ScoringPreferences;
  const dirty = valid && !sameScoring(values, defaults);
  const builtInTexts = textsOf(SCORING_DEFAULTS);
  const atBuiltIn = sameScoring(defaults, SCORING_DEFAULTS) && sameTexts(texts, builtInTexts) && !edited;

  const adopt = (next: ScoringPreferences) => {
    setBase(next);
    setTexts(textsOf(next));
    setUnreadable({});
  };

  const save = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    // A field holding text the browser cannot read reports no change at
    // all, so look before saving.
    const unread: Partial<Record<ScoringField, boolean>> = {};
    for (const input of Array.from(event.currentTarget.querySelectorAll('input'))) {
      if (input.validity.badInput && input.dataset.field) {
        unread[input.dataset.field as ScoringField] = true;
      }
    }
    if (Object.keys(unread).length > 0) {
      setUnreadable(unread);
      return;
    }
    if (!dirty || !canWrite || saveDefaults.isPending) return;
    // The fields keep what was typed until the server has it: a failed
    // save leaves the edits to try again.
    saveDefaults.mutate(values, {
      onSuccess: (saved) => adopt(scoringPreferencesOf(saved.scoring)),
    });
  };

  const field = (key: ScoringField) => {
    const rule = SCORING_RULES[key];
    const id = `${fieldId}-${key}`;
    const invalid = parsed[key] === undefined;
    const builtIn = formatScoringValue(key, SCORING_DEFAULTS[key]);
    return (
      <div key={key} className="scoring-default">
        <div className="scoring-default-text">
          <label htmlFor={id} className="scoring-default-label">
            {rule.label}
            {rule.unit && <>{' '}<span className="visually-hidden">({rule.unit})</span></>}
          </label>
          <small id={`${id}-hint`}>{HINTS[key]} Built in: {builtIn}.</small>
        </div>
        <span className="scoring-default-input">
          <input
            id={id}
            data-field={key}
            type="number"
            inputMode="decimal"
            min={rule.percent ? rule.min * 100 : rule.min}
            max={rule.percent ? rule.max * 100 : rule.max}
            step={rule.percent ? Math.round((rule.step ?? 0) * 100) : rule.step ?? 'any'}
            placeholder={rule.optional ? 'off' : undefined}
            value={texts[key]}
            aria-invalid={invalid}
            aria-describedby={invalid ? `${id}-hint ${id}-error` : `${id}-hint`}
            onChange={(event) => {
              const bad = event.target.validity.badInput;
              setUnreadable((current) => ({ ...current, [key]: bad }));
              if (!bad) setTexts((current) => ({ ...current, [key]: event.target.value }));
            }}
          />
          <span className="scoring-default-unit" aria-hidden="true">{UNIT_MARKS[key]}</span>
        </span>
        {invalid && (
          <p id={`${id}-error`} className="scoring-default-error" role="alert">
            {rule.error}
          </p>
        )}
      </div>
    );
  };

  return (
    <section className="scoring-defaults" aria-labelledby={headingId}>
      <UnsavedMark label="scoring defaults" dirty={edited} />
      <h3 id={headingId}>Sequence scoring</h3>
      <p className="scoring-defaults-intro">
        Every view's <strong>Scoring</strong> control starts from these, for
        everyone on this server, and automatic stack builds score with them. A
        change in a view's control applies to that view until you save it as
        the default. When automatic stack previews are on, saving new defaults
        rebuilds stacks scored another way, after the arrival delay set under
        Settings › Stacking.
      </p>
      {!canWrite && (
        <p className="scoring-defaults-intro">Only an editor can change the defaults.</p>
      )}
      {newerArrived && (
        <p className="scoring-defaults-newer" role="status">
          Newer defaults were saved elsewhere while you were editing. Saving will replace them with your edits.{' '}
          <button type="button" onClick={() => adopt(defaults)}>
            Show the newer defaults
          </button>
        </p>
      )}
      <form className="scoring-defaults-form" onSubmit={save} noValidate>
        <fieldset className="scoring-defaults-group" disabled={!canWrite}>
          <legend>Penalties</legend>
          {PENALTIES.map(field)}
        </fieldset>
        <fieldset className="scoring-defaults-group" disabled={!canWrite}>
          <legend>Reject limits</legend>
          {LIMITS.map(field)}
        </fieldset>
        {/* aria-disabled, not disabled: a button that disables itself as it
            is pressed would drop keyboard focus out of the form. */}
        <div className="scoring-defaults-actions">
          <button type="submit" className="save-button" aria-disabled={!dirty || !canWrite || saveDefaults.isPending}>
            {saveDefaults.isPending ? 'Saving…' : 'Save defaults'}
          </button>
          <button
            type="button"
            className="cancel-button"
            aria-disabled={atBuiltIn || !canWrite}
            onClick={() => {
              if (atBuiltIn || !canWrite || saveDefaults.isPending) return;
              saveDefaults.mutate(
                { ...SCORING_DEFAULTS },
                { onSuccess: (saved) => adopt(scoringPreferencesOf(saved.scoring)) },
              );
            }}
          >
            Reset to built-in defaults
          </button>
        </div>
        {saveDefaults.isError && (
          <p className="scoring-default-error" role="alert">
            Could not save the defaults: {(saveDefaults.error as Error).message}
          </p>
        )}
      </form>
    </section>
  );
}
