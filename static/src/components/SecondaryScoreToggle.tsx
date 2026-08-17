import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../hooks/useDisplayPreferences';

/**
 * Quick toggle for the second score chip on cards. One shared preference:
 * flipping it here changes the grid, the Sequence view, and the detail
 * panel together.
 */
export default function SecondaryScoreToggle({ className }: { className?: string }) {
  const preferences = useDisplayPreferences();
  return (
    <label
      className={`secondary-score-toggle${className ? ` ${className}` : ''}`}
      title='Show the other score basis ("night" vs "all") as a second chip when it disagrees with the main badge'
    >
      <input
        type="checkbox"
        checked={preferences.showSecondaryScore}
        onChange={(event) =>
          setDisplayPreferences({
            ...preferences,
            showSecondaryScore: event.target.checked,
          })
        }
      />
      2nd score
    </label>
  );
}
