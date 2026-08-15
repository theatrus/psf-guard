import { useState } from 'react';
import {
  setScoringPreferences,
  useScoringPreferences,
} from '../hooks/useScoringPreferences';
import type { ScoringPreferences } from '../hooks/useScoringPreferences';

/**
 * Compact control for how hard each kind of event evidence hits the quality
 * score. One shared, remembered preference: the Sequence view, grid badges,
 * and detail panel all score with the same scales.
 *
 * Sliders track locally while dragging and commit on release: every commit
 * changes the scoring query keys and refetches whole-scope analyses, so
 * per-step commits would fire one full analysis per 0.1 tick of a drag.
 */
export default function ScoringPenaltyControl() {
  const preferences = useScoringPreferences();
  const [draft, setDraft] = useState<ScoringPreferences | null>(null);
  const shown = draft ?? preferences;
  const isDefault = shown.satellite === 1 && shown.pointing === 1 && shown.temporal === 1;

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
      <span className="penalty-value">{Math.round(shown[key] * 100)}%</span>
    </label>
  );

  return (
    <details className="scoring-penalty-control">
      <summary title="How hard each kind of evidence lowers the quality score. 100% is the calibrated default; 0% ignores that evidence; 200% doubles the hit.">
        Penalties{isDefault ? '' : ' *'}
      </summary>
      <div className="scoring-penalty-panel">
        {slider(
          'satellite',
          'Satellite trails',
          'Score hit for a pixel-confirmed satellite trail crossing the frame. At 0% the trail also stops driving reject recommendations.'
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
        <button
          type="button"
          className="penalty-reset"
          disabled={isDefault}
          onClick={() => {
            setDraft(null);
            setScoringPreferences({ satellite: 1, pointing: 1, temporal: 1 });
          }}
        >
          Reset to defaults
        </button>
      </div>
    </details>
  );
}
