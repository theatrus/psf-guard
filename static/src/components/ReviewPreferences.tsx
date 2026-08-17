import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../hooks/useDisplayPreferences';
import { LayersIcon, MoonIcon } from './ScoreChipIcons';

/**
 * Review behavior preferences. Stored in this browser and shared by the
 * grid, the Sequence view, and the detail view, so every surface behaves
 * the same way.
 */
export default function ReviewPreferences() {
  const preferences = useDisplayPreferences();
  const set = (patch: Partial<typeof preferences>) =>
    setDisplayPreferences({ ...preferences, ...patch });

  return (
    <div className="review-preferences">
      <h3>Grading</h3>
      <label className="review-preference">
        <input
          type="checkbox"
          checked={preferences.advanceOnGrade}
          onChange={(event) => set({ advanceOnGrade: event.target.checked })}
        />
        <span>
          Move to the next image after accept, reject, or pending
          <small>
            Holding Shift while grading does the opposite of this setting for
            that one grade.
          </small>
        </span>
      </label>

      <h3>Score chips</h3>
      <label className="review-preference">
        <input
          type="checkbox"
          checked={preferences.showNightChip}
          onChange={(event) => set({ showNightChip: event.target.checked })}
        />
        <span>
          <MoonIcon /> Night-session score chip
          <small>How the frame ranks within its own capture session.</small>
        </span>
      </label>
      <label className="review-preference">
        <input
          type="checkbox"
          checked={preferences.showAllChip}
          onChange={(event) => set({ showAllChip: event.target.checked })}
        />
        <span>
          <LayersIcon /> All-sessions score chip
          <small>
            How the frame ranks against every stack candidate for its filter.
          </small>
        </span>
      </label>

      <p className="review-preferences-note">
        These preferences live in this browser and apply immediately.
      </p>
    </div>
  );
}
