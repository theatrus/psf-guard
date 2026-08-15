import {
  setScoringPreferences,
  useScoringPreferences,
} from '../hooks/useScoringPreferences';

/**
 * Compact control for how hard each kind of event evidence hits the quality
 * score. One shared, remembered preference: the Sequence view, grid badges,
 * and detail panel all score with the same scales.
 */
export default function ScoringPenaltyControl() {
  const preferences = useScoringPreferences();
  const isDefault =
    preferences.satellite === 1 && preferences.pointing === 1 && preferences.temporal === 1;

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
        value={preferences[key]}
        onChange={(event) =>
          setScoringPreferences({ ...preferences, [key]: parseFloat(event.target.value) })
        }
      />
      <span className="penalty-value">{Math.round(preferences[key] * 100)}%</span>
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
          'Score hit for a pixel-confirmed satellite trail crossing the frame.'
        )}
        {slider(
          'pointing',
          'Pointing',
          'Score hit for off-target, pointing-jump, and pointing-drift evidence.'
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
          onClick={() => setScoringPreferences({ satellite: 1, pointing: 1, temporal: 1 })}
        >
          Reset to defaults
        </button>
      </div>
    </details>
  );
}
