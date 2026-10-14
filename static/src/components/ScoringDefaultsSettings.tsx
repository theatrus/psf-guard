import { useId, useState, type FormEvent } from 'react';
import { setScoringDefaults, useScoringDefaults } from '../hooks/useScoringPreferences';
import {
  formatScoringValue,
  parseScoringText,
  SCORING_DEFAULTS,
  SCORING_RULES,
  sameScoring,
  scoringText,
  type ScoringField,
  type ScoringPreferences,
} from '../utils/scoring';

type Texts = Record<ScoringField, string>;

const HINTS: Record<ScoringField, string> = {
  satellite: 'The score hit for a pixel-confirmed trail. At 0% trails neither lower the score nor drive reject recommendations.',
  pointing: 'The score hit for off-target, pointing-jump and drift evidence, and the solve failures it corroborates.',
  temporal: 'The score hit when a frame suddenly departs from its neighbours.',
  hfrRejectAbove: 'Recommend rejecting a frame whose measured HFR is over this. Empty turns it off.',
  starCountRejectBelow: 'Recommend rejecting a frame with fewer detected stars than this. Empty turns it off.',
};

/** The unit shown beside a field; its name is part of the field's. */
const UNIT_MARKS: Partial<Record<ScoringField, string>> = {
  satellite: '%',
  pointing: '%',
  temporal: '%',
  hfrRejectAbove: 'px',
};

const PENALTIES: ScoringField[] = ['satellite', 'pointing', 'temporal'];
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
 * Settings › Review › Sequence scoring: the person's own defaults for the
 * Scoring control. Every view starts from them; a change in a view's
 * toolbar stays in that view until it is saved as the default. They are
 * kept with the other review preferences: in this browser, or with a
 * signed-in user's account. Each field follows the same rules as the
 * toolbar and the link (SCORING_RULES), so a default saved anywhere opens
 * here as a value the form accepts.
 */
export default function ScoringDefaultsSettings() {
  const headingId = useId();
  const fieldId = useId();
  const defaults = useScoringDefaults();
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
    if (!dirty) return;
    adopt(values);
    setScoringDefaults(values);
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
      <h3 id={headingId}>Sequence scoring</h3>
      <p className="scoring-defaults-intro">
        Every view's <strong>Scoring</strong> control starts from these. A
        change there applies to that view until you save it as the default.
      </p>
      {newerArrived && (
        <p className="scoring-defaults-newer" role="status">
          Newer defaults were saved elsewhere while you were editing. Saving
          replaces them with what is here.{' '}
          <button type="button" onClick={() => adopt(defaults)}>
            Show the newer defaults
          </button>
        </p>
      )}
      <form className="scoring-defaults-form" onSubmit={save} noValidate>
        <fieldset className="scoring-defaults-group">
          <legend>Penalties</legend>
          {PENALTIES.map(field)}
        </fieldset>
        <fieldset className="scoring-defaults-group">
          <legend>Reject limits</legend>
          {LIMITS.map(field)}
        </fieldset>
        {/* aria-disabled, not disabled: a button that disables itself as it
            is pressed would drop keyboard focus out of the form. */}
        <div className="scoring-defaults-actions">
          <button type="submit" className="save-button" aria-disabled={!dirty}>
            Save defaults
          </button>
          <button
            type="button"
            className="cancel-button"
            aria-disabled={atBuiltIn}
            onClick={() => {
              if (atBuiltIn) return;
              adopt({ ...SCORING_DEFAULTS });
              setScoringDefaults({ ...SCORING_DEFAULTS });
            }}
          >
            Reset to built-in defaults
          </button>
        </div>
      </form>
    </section>
  );
}
