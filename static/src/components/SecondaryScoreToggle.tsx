import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../hooks/useDisplayPreferences';
import { CalibrationIcon, LayersIcon, MoonIcon } from './ScoreChipIcons';

/**
 * Quick toggles for the chips on cards, labeled with the same icons the
 * chips carry. One shared preference: flipping a box changes
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
      <label title="Show the calibration chip on lights the calibration library serves poorly or that are worth a look">
        <input
          type="checkbox"
          checked={preferences.showCalibrationChip}
          onChange={(event) =>
            setDisplayPreferences({
              ...preferences,
              showCalibrationChip: event.target.checked,
            })
          }
        />
        <CalibrationIcon />
      </label>
    </div>
  );
}
