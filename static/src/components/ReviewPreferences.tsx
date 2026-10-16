import { useAccess } from '../auth/access';
import {
  setDisplayPreferences,
  TIME_MARKS_CHOICES,
  TIME_MARKS_LABELS,
  useDisplayPreferences,
  type TimeMarksChoice as TimeMarksChoiceValue,
} from '../hooks/useDisplayPreferences';
import { CalibrationIcon, LayersIcon, MoonIcon } from './ScoreChipIcons';
import ScoringDefaultsSettings from './ScoringDefaultsSettings';

/**
 * Review behavior preferences, shared by the grid, the Sequence view, and
 * the detail view, so every surface behaves the same way. A signed-in
 * user's follow their account (see useAccountPreferences); otherwise this
 * browser keeps them.
 */
export default function ReviewPreferences() {
  const preferences = useDisplayPreferences();
  const { status } = useAccess();
  const set = (patch: Partial<typeof preferences>) =>
    setDisplayPreferences({ ...preferences, ...patch });

  return (
    <>
      <section className="review-preferences" aria-label="Grading and chips">
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

        <fieldset className="settings-group">
          <legend>Score chips</legend>
          <label className="review-preference">
            <input
              type="checkbox"
              checked={preferences.showNightChip}
              onChange={(event) => set({ showNightChip: event.target.checked })}
            />
            <span>
              <span className="review-preference-title"><MoonIcon /> Night-session score chip</span>
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
              <span className="review-preference-title"><LayersIcon /> All-sessions score chip</span>
              <small>
                How the frame ranks against every stack candidate for its filter.
              </small>
            </span>
          </label>
          <label className="review-preference">
            <input
              type="checkbox"
              checked={preferences.showCalibrationChip}
              onChange={(event) => set({ showCalibrationChip: event.target.checked })}
            />
            <span>
              <span className="review-preference-title"><CalibrationIcon /> Calibration chip</span>
              <small>
                Red on a light the calibration library can't calibrate properly,
                yellow on one worth a look.
              </small>
            </span>
          </label>
        </fieldset>
      </section>

      <ScoringDefaultsSettings />

      <section className="review-preferences" aria-label="Display">
        <h3>Display</h3>
        <fieldset className="settings-group">
          <legend>Images and Sequence</legend>
          <label className="review-preference">
            <span>
              Night and hour marks
              <small>
                Lines where frames cross into another night (at each rig's local
                noon) or hour. The View panel changes this too.
              </small>
            </span>
            <select
              value={preferences.timeMarks ?? 'nights-hours'}
              aria-label="Night and hour marks"
              onChange={(event) => set({
                timeMarks: (TIME_MARKS_CHOICES as readonly string[]).includes(event.target.value)
                  ? event.target.value as TimeMarksChoiceValue
                  : 'nights-hours',
              })}
            >
              {TIME_MARKS_CHOICES.map((choice) => (
                <option key={choice} value={choice}>{TIME_MARKS_LABELS[choice]}</option>
              ))}
            </select>
          </label>
        </fieldset>

        <fieldset className="settings-group">
          <legend>Top bar</legend>
          <label className="review-preference">
            <span>
              Project list grouping
              <small>
                By how recently each project was worked, or one group per
                database.
              </small>
            </span>
            <select
              value={preferences.projectPickerGrouping}
              aria-label="Project list grouping"
              onChange={(event) =>
                set({
                  projectPickerGrouping:
                    event.target.value === 'database' ? 'database' : 'activity',
                })
              }
            >
              <option value="activity">By recent activity</option>
              <option value="database">By database</option>
            </select>
          </label>
        </fieldset>

        <fieldset className="settings-group">
          <legend>Library</legend>
          <label className="review-preference">
            <span>
              Projects open as
              <small>
                Compact rows show each project as a line of pills; any row opens
                into its full card. The Library's toggle changes this too.
              </small>
            </span>
            <select
              value={preferences.libraryDensity}
              aria-label="Library project view"
              onChange={(event) =>
                set({ libraryDensity: event.target.value === 'detailed' ? 'detailed' : 'compact' })
              }
            >
              <option value="compact">Compact rows</option>
              <option value="detailed">Full cards</option>
            </select>
          </label>
        </fieldset>

        <p className="review-preferences-note">
          {status.username
            ? `These preferences apply at once and are saved with your account, ${status.username}, so they follow you to any browser.`
            : 'These preferences live in this browser and apply at once.'}
        </p>
      </section>
    </>
  );
}
