import type { AutoImportSettings, ImportScope } from '../api/types';
import { AUTOIMPORT_INTERVALS } from '../utils/autoimport';

interface AutoImportFieldsProps {
  value: AutoImportSettings;
  onChange: (next: AutoImportSettings) => void;
}

/**
 * The automatic import block of the database form. Off by default; when on,
 * the run happens on open, on a schedule, or both, over the chosen frame
 * kinds.
 */
export default function AutoImportFields({ value, onChange }: AutoImportFieldsProps) {
  const set = (patch: Partial<AutoImportSettings>) => onChange({ ...value, ...patch });
  const intervalListed = AUTOIMPORT_INTERVALS.some(
    (choice) => choice.minutes === value.interval_minutes
  );
  return (
    <fieldset className="autoimport-fields">
      <legend>Automatic import</legend>
      <label className="checkbox-label">
        <input
          type="checkbox"
          checked={value.enabled}
          onChange={(event) => set({ enabled: event.target.checked })}
        />{' '}
        Import new frames from the image directories on their own
      </label>
      {value.enabled && (
        <>
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={value.on_open}
              onChange={(event) => set({ on_open: event.target.checked })}
            />{' '}
            Run when the app or server opens this database
          </label>
          <label htmlFor="autoimport-interval">Schedule:</label>
          <select
            id="autoimport-interval"
            className="file-path-input"
            value={value.interval_minutes}
            onChange={(event) => set({ interval_minutes: Number(event.target.value) })}
          >
            {AUTOIMPORT_INTERVALS.map((choice) => (
              <option key={choice.minutes} value={choice.minutes}>
                {choice.label}
              </option>
            ))}
            {!intervalListed && (
              <option value={value.interval_minutes}>
                Every {value.interval_minutes} minutes
              </option>
            )}
          </select>
          <label htmlFor="autoimport-scope">Import:</label>
          <select
            id="autoimport-scope"
            className="file-path-input"
            value={value.scope}
            onChange={(event) => set({ scope: event.target.value as ImportScope })}
          >
            <option value="all">Lights and calibration frames</option>
            <option value="lights">Lights only</option>
            <option value="calibration">Calibration frames only</option>
          </select>
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={value.backfill}
              onChange={(event) => set({ backfill: event.target.checked })}
            />{' '}
            Analyze quality of the new frames in the background
          </label>
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={value.accept_other_rigs}
              onChange={(event) => set({ accept_other_rigs: event.target.checked })}
            />{' '}
            Include frames from other rigs
          </label>
          {!value.on_open && value.interval_minutes === 0 && (
            <small className="autoimport-warning">
              Choose a run on open, a schedule, or both.
            </small>
          )}
          <small className="muted">
            A run reads only files the catalog does not have yet, with the same
            matching rules as the Import button, and never asks for confirmation.
          </small>
        </>
      )}
    </fieldset>
  );
}
