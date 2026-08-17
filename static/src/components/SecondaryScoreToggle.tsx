import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../hooks/useDisplayPreferences';
import { LayersIcon, MoonIcon } from './ScoreChipIcons';

/**
 * Quick toggles for the two score chips on cards, labeled with the same
 * icons the chips carry. One shared preference: flipping a box changes
 * the grid, the Sequence view, and the detail panel together.
 */
export default function SecondaryScoreToggle({ className }: { className?: string }) {
  const preferences = useDisplayPreferences();
  return (
    <div className={`secondary-score-toggle${className ? ` ${className}` : ''}`}>
      <label title="Show the night-session score chip: the frame compared within its own capture session">
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
        <MoonIcon />
      </label>
      <label title="Show the all-sessions score chip: the frame compared across every stack candidate for its filter">
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
        <LayersIcon />
      </label>
    </div>
  );
}
