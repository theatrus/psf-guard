import { useId, useState } from 'react';
import { useAccess } from '../auth/access';
import { useViewScoring } from '../hooks/useScoringPreferences';
import {
  formatLimit,
  formatPenalty,
  MAX_SCORING_PENALTY,
  normalizeScoring,
  parseScoringText,
  sameScoring,
  SCORING_PENALTY_STEP,
  SCORING_RULES,
  type LimitKey,
  type PenaltyKey,
  type ScoringPreferences,
} from '../utils/scoring';
import { openSettings } from '../utils/settingsIntent';

const LIMIT_KEYS: LimitKey[] = ['hfrRejectAbove', 'starCountRejectBelow'];

/**
 * The Sequence toolbar's Scoring control: how hard each kind of event
 * evidence hits the quality score, plus the absolute reject limits (HFR
 * ceiling, star-count floor).
 *
 * It starts from the person's defaults (Settings › Review). A change here
 * applies to this view: it goes in the link, so it stays through a reload
 * and travels to Images, the detail view, Stacks and the project's other
 * targets, which say so, and the button says the view differs. Save as
 * default makes it the default; Reset to default drops it.
 *
 * Inputs track locally while editing and commit on release/blur: every
 * commit changes the scoring query keys and refetches whole-scope analyses,
 * so per-step commits would fire one full analysis per 0.1 tick of a drag
 * or per typed digit. The limit drafts stay raw text until commit so
 * partial entries like "0" on the way to "0.8" survive. A limit the
 * shared rules refuse (utils/scoring) is not committed: it shows why once
 * the person leaves it or presses Enter. A committed draft stays shown
 * until the link carries it, so the old value never flashes back while the
 * router catches up.
 */
export default function ScoringPenaltyControl() {
  const errorId = useId();
  const { scoring, defaults, setScoring, saveAsDefault, resetToDefault } = useViewScoring();
  // Settings opens for editors only; a viewer saves defaults from here.
  const { canWrite } = useAccess();
  const [draft, setDraft] = useState<ScoringPreferences | null>(null);
  const [limitTexts, setLimitTexts] = useState<Partial<Record<LimitKey, string>>>({});
  // The limits whose refused text the person tried to commit.
  const [tried, setTried] = useState<Partial<Record<LimitKey, boolean>>>({});
  // The scoring the drafts were made against. Once the link changes, by a
  // commit landing or by navigation, the drafts give way to it.
  const [base, setBase] = useState(scoring);
  if (!sameScoring(base, scoring)) {
    setBase(scoring);
    setDraft(null);
    setLimitTexts({});
    setTried({});
  }
  const shown = draft ?? scoring;

  /** A limit's typed value: the value, null for off, or undefined when the
   *  text is not one it may hold. */
  const typed = (key: LimitKey): number | null | undefined => {
    const text = limitTexts[key];
    return text === undefined ? shown[key] : parseScoringText(key, text);
  };
  const refused = LIMIT_KEYS.filter((key) => typed(key) === undefined);
  /** A refused limit counts as what it was until it is fixed. */
  const settled = (key: LimitKey): number | null => {
    const value = typed(key);
    return value === undefined ? shown[key] : value;
  };
  const effective: ScoringPreferences = {
    ...shown,
    hfrRejectAbove: settled('hfrRejectAbove'),
    starCountRejectBelow: settled('starCountRejectBelow'),
  };
  const differs = !sameScoring(effective, defaults);
  const canSave = differs && refused.length === 0;

  const clearDrafts = () => {
    setDraft(null);
    setLimitTexts({});
    setTried({});
  };
  const commit = () => {
    if (draft === null && Object.keys(limitTexts).length === 0) return;
    if (refused.length > 0) {
      setTried(Object.fromEntries(refused.map((key) => [key, true])));
      return;
    }
    const next = normalizeScoring(effective, scoring);
    if (sameScoring(next, scoring)) {
      clearDrafts();
      return;
    }
    // The drafts stay until the link carries the new values.
    setScoring(next);
  };

  const slider = (key: PenaltyKey, title: string) => {
    const off = shown[key] !== defaults[key];
    return (
      <label
        className={`penalty-slider${off ? ' is-set' : ''}`}
        title={`${title} Your default: ${formatPenalty(defaults[key])}.`}
      >
        <span className="penalty-label">{SCORING_RULES[key].label}</span>
        <input
          type="range"
          min="0"
          max={MAX_SCORING_PENALTY}
          step={SCORING_PENALTY_STEP}
          value={shown[key]}
          onChange={(event) =>
            setDraft({ ...shown, [key]: parseFloat(event.target.value) })
          }
          onPointerUp={commit}
          onKeyUp={commit}
          onBlur={commit}
        />
        <span className="penalty-value">{formatPenalty(shown[key])}</span>
      </label>
    );
  };

  const limitInput = (key: LimitKey, title: string) => {
    const rule = SCORING_RULES[key];
    const value = typed(key);
    const off = value !== undefined && value !== defaults[key];
    const showError = tried[key] && value === undefined;
    return (
      <>
        <label
          className={`penalty-limit${off ? ' is-set' : ''}`}
          title={`${title} Your default: ${formatLimit(defaults[key])}.`}
        >
          <span className="penalty-label">{rule.label}</span>
          <input
            type="number"
            min={rule.optional ? 0 : rule.min}
            max={rule.max}
            step={rule.step ?? 'any'}
            placeholder="off"
            value={limitTexts[key] ?? shown[key] ?? ''}
            aria-invalid={showError || undefined}
            aria-describedby={showError ? `${errorId}-${key}` : undefined}
            onChange={(event) => {
              // A number input reports '' for text it cannot parse ("e",
              // "3.") and flags it as badInput. Recording that '' would
              // commit as "clear the limit" on blur, so keep the previous
              // draft instead.
              if (event.target.validity.badInput) return;
              setTried((current) => ({ ...current, [key]: false }));
              setLimitTexts((current) => ({ ...current, [key]: event.target.value }));
            }}
            onBlur={commit}
            onKeyUp={(event) => {
              if (event.key === 'Enter') commit();
            }}
          />
        </label>
        {showError && (
          <p id={`${errorId}-${key}`} className="penalty-error" role="alert">
            {rule.error}
          </p>
        )}
      </>
    );
  };

  return (
    <details className={`scoring-penalty-control${differs ? ' is-set' : ''}`}>
      <summary title="How evidence and limits affect the quality score and reject recommendations. Penalties: 100% is the calibrated default, 0% ignores that evidence, 200% doubles the hit. Reject limits are fixed cut-offs like N.I.N.A. subframe selection. Changes here apply to this view until you save them as your default.">
        Scoring{differs ? ' (changed)' : ''}
      </summary>
      <div className="scoring-penalty-panel">
        <span className="penalty-section">Penalties</span>
        {slider(
          'satellite',
          'Score hit for a pixel-confirmed satellite trail crossing the frame. Off (0%) means trails neither lower the score nor drive reject recommendations - useful when trails are removed during stacking.'
        )}
        {slider(
          'pointing',
          'Score hit for off-target, pointing-jump, pointing-drift, and corroborated solve-failure evidence. At 0% pointing evidence also stops driving reject recommendations.'
        )}
        {slider(
          'temporal',
          'Score hit when a frame suddenly deviates from its neighbours.'
        )}
        <span className="penalty-section">Reject limits</span>
        {limitInput(
          'hfrRejectAbove',
          'Recommend rejecting any frame whose measured HFR (pixels) is over this value, however the rest of the sequence looks. Empty turns the limit off.'
        )}
        {limitInput(
          'starCountRejectBelow',
          'Recommend rejecting any frame with fewer detected stars than this, however the rest of the sequence looks. Empty turns the limit off.'
        )}
        <p className="penalty-defaults-note">
          {differs ? 'Changed for this view.' : 'Your defaults.'}
          {canWrite && (
            <>
              {' '}
              <button
                type="button"
                className="penalty-settings-link"
                onClick={() => openSettings('review')}
              >
                Edit defaults
              </button>
            </>
          )}
        </p>
        {/* aria-disabled, not disabled: a button that disables itself as
            it is pressed would drop keyboard focus out of the popover. */}
        <div className="penalty-actions">
          <button
            type="button"
            className="tool-button"
            aria-disabled={!differs}
            title="Score this view with your defaults again"
            onClick={() => {
              if (!differs) return;
              clearDrafts();
              resetToDefault();
            }}
          >
            Reset to default
          </button>
          <button
            type="button"
            className="tool-button"
            aria-disabled={!canSave}
            title="Make these values your defaults: every view starts from them"
            onClick={() => {
              if (!canSave) {
                if (refused.length > 0) {
                  setTried(Object.fromEntries(refused.map((key) => [key, true])));
                }
                return;
              }
              const next = normalizeScoring(effective, scoring);
              clearDrafts();
              saveAsDefault(next);
            }}
          >
            Save as default
          </button>
        </div>
      </div>
    </details>
  );
}
