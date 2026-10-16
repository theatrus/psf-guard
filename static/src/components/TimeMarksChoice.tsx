import {
  setDisplayPreferences,
  TIME_MARKS_CHOICES,
  TIME_MARKS_LABELS,
  useDisplayPreferences,
  type TimeMarksChoice as Choice,
} from '../hooks/useDisplayPreferences';

/**
 * The View panel's switch for the night and hour marks between cards. One
 * preference with Settings › Review, so Images and Sequence change
 * together.
 */
export default function TimeMarksChoice({ id }: { id: string }) {
  const preferences = useDisplayPreferences();
  return (
    <div className="toolbar-field">
      <label className="toolbar-caption" htmlFor={id}>Marks</label>
      <select
        id={id}
        value={preferences.timeMarks ?? 'nights-hours'}
        title="Lines between cards where frames cross into another night, at the rig's local noon, or another hour"
        onChange={(event) => setDisplayPreferences({
          ...preferences,
          timeMarks: (TIME_MARKS_CHOICES as readonly string[]).includes(event.target.value)
            ? event.target.value as Choice
            : 'nights-hours',
        })}
      >
        {TIME_MARKS_CHOICES.map((choice) => (
          <option key={choice} value={choice}>{TIME_MARKS_LABELS[choice]}</option>
        ))}
      </select>
    </div>
  );
}
