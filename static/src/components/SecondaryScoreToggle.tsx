import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../hooks/useDisplayPreferences';

/**
 * Quick toggles for the second score chip on cards, one per chip type.
 * One shared preference: flipping a box here changes the grid, the
 * Sequence view, and the detail panel together.
 */
export default function SecondaryScoreToggle({ className }: { className?: string }) {
  const preferences = useDisplayPreferences();
  return (
    <div className={`secondary-score-toggle${className ? ` ${className}` : ''}`}>
      <span className="secondary-score-toggle-label">Chips:</span>
      <label title='Show the "night <score>" chip — how the frame ranks within its own session — when the main badge is the all-sessions score'>
        <input
          type="checkbox"
          checked={preferences.showNightChip}
          onChange={(event) =>
            setDisplayPreferences({
              ...preferences,
              showNightChip: event.target.checked,
            })
          }
        />
        night
      </label>
      <label title='Show the "all <score>" chip — how the frame ranks against every stack candidate for its filter — when the main badge is a single session score'>
        <input
          type="checkbox"
          checked={preferences.showAllChip}
          onChange={(event) =>
            setDisplayPreferences({
              ...preferences,
              showAllChip: event.target.checked,
            })
          }
        />
        all
      </label>
    </div>
  );
}
