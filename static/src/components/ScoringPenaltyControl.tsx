import { useState } from 'react';
import {
  setScoringPreferences,
  useScoringPreferences,
} from '../hooks/useScoringPreferences';
import type { ScoringPreferences } from '../hooks/useScoringPreferences';

const DEFAULT_PREFERENCES: ScoringPreferences = {
  satellite: 1,
  pointing: 1,
  temporal: 1,
  hfrRejectAbove: null,
  starCountRejectBelow: null,
};

/**
 * Compact control for the shared scoring preferences: how hard each kind of
 * event evidence hits the quality score, plus the operator's absolute
 * reject limits (HFR ceiling, star-count floor). One shared, remembered
 * preference: the Sequence view, grid badges, and detail panel all score
 * with the same settings.
 *
 * Inputs track locally while editing and commit on release/blur: every
 * commit changes the scoring query keys and refetches whole-scope analyses,
 * so per-step commits would fire one full analysis per 0.1 tick of a drag
 * or per typed digit.
 */
export default function ScoringPenaltyControl() {
  const preferences = useScoringPreferences();
  const [draft, setDraft] = useState<ScoringPreferences | null>(null);
  const shown = draft ?? preferences;
  const isDefault =
    shown.satellite === 1 &&
    shown.pointing === 1 &&
    shown.temporal === 1 &&
    shown.hfrRejectAbove == null &&
    shown.starCountRejectBelow == null;

  const commit = () => {
    if (draft) {
      setScoringPreferences(draft);
      setDraft(null);
    }
  };

  const slider = (
    key: 'satellite' | 'pointing' | 'temporal',
    label: string,
    title: string
  ) => (
    <label className="penalty-slider" title={title}>
      <span className="penalty-label">{label}</span>
      <input
        type="range"
        min="0"
        max="2"
        step="0.1"
        value={shown[key]}
        onChange={(event) =>
          setDraft({ ...shown, [key]: parseFloat(event.target.value) })
        }
        onPointerUp={commit}
        onKeyUp={commit}
        onBlur={commit}
      />
      <span className="penalty-value">
        {shown[key] === 0 ? 'off' : `${Math.round(shown[key] * 100)}%`}
      </span>
    </label>
  );

  const limitInput = (
    key: 'hfrRejectAbove' | 'starCountRejectBelow',
    label: string,
    title: string,
    step: string
  ) => (
    <label className="penalty-limit" title={title}>
      <span className="penalty-label">{label}</span>
      <input
        type="number"
        min="0"
        step={step}
        placeholder="off"
        value={shown[key] ?? ''}
        onChange={(event) => {
          const parsed = parseFloat(event.target.value);
          setDraft({
            ...shown,
            [key]: Number.isFinite(parsed) && parsed > 0 ? parsed : null,
          });
        }}
        onBlur={commit}
        onKeyUp={(event) => {
          if (event.key === 'Enter') commit();
        }}
      />
    </label>
  );

  return (
    <details className="scoring-penalty-control">
      <summary title="How evidence and limits affect the quality score and reject recommendations. Penalties: 100% is the calibrated default, 0% ignores that evidence, 200% doubles the hit. Reject limits are absolute thresholds like N.I.N.A. subframe selection.">
        Scoring{isDefault ? '' : ' *'}
      </summary>
      <div className="scoring-penalty-panel">
        <span className="penalty-section">Penalties</span>
        {slider(
          'satellite',
          'Satellite trails',
          'Score hit for a pixel-confirmed satellite trail crossing the frame. Off (0%) means trails neither lower the score nor drive reject recommendations - useful when trails are removed during stacking.'
        )}
        {slider(
          'pointing',
          'Pointing',
          'Score hit for off-target, pointing-jump, pointing-drift, and corroborated solve-failure evidence. At 0% pointing evidence also stops driving reject recommendations.'
        )}
        {slider(
          'temporal',
          'Temporal anomalies',
          'Score hit when a frame suddenly deviates from its neighbours.'
        )}
        <span className="penalty-section">Reject limits</span>
        {limitInput(
          'hfrRejectAbove',
          'HFR above',
          'Recommend rejecting any frame whose measured HFR exceeds this value (pixels), regardless of sequence context. Empty turns the limit off.',
          '0.1'
        )}
        {limitInput(
          'starCountRejectBelow',
          'Stars below',
          'Recommend rejecting any frame with fewer detected stars than this, regardless of sequence context. Empty turns the limit off.',
          '1'
        )}
        <button
          type="button"
          className="penalty-reset"
          disabled={isDefault}
          onClick={() => {
            setDraft(null);
            setScoringPreferences({ ...DEFAULT_PREFERENCES });
          }}
        >
          Reset to defaults
        </button>
      </div>
    </details>
  );
}
