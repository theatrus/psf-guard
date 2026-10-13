import type { ScanRules } from '../api/types';

interface ScanRulesFieldsProps {
  value: ScanRules;
  onChange: (next: ScanRules) => void;
}

/** One pattern per line; blank lines are dropped when the server saves. */
const lines = (text: string) => text.split('\n');

/**
 * The scan rules block of the database form: what every import from the
 * image folders leaves out. A `.psfguardignore` file skips its folder
 * whatever is set here.
 */
export default function ScanRulesFields({ value, onChange }: ScanRulesFieldsProps) {
  const set = (patch: Partial<ScanRules>) => onChange({ ...value, ...patch });
  return (
    <fieldset className="autoimport-fields scan-rules-fields">
      <legend>Scan rules</legend>
      <label htmlFor="scan-rules-exclude">Exclude:</label>
      <textarea
        id="scan-rules-exclude"
        className="file-path-input scan-rules-patterns"
        rows={3}
        spellCheck={false}
        placeholder={'**/Hocus Focus/**\n**/PierCam/**'}
        title="One pattern per line. A pattern that matches a folder skips everything below it."
        value={value.exclude.join('\n')}
        onChange={(event) => set({ exclude: lines(event.target.value) })}
      />
      <label htmlFor="scan-rules-include">Include only:</label>
      <textarea
        id="scan-rules-include"
        className="file-path-input scan-rules-patterns"
        rows={2}
        spellCheck={false}
        placeholder="LIGHT"
        title="One pattern per line. When set, only files that match one, or sit in a folder that does, are read."
        value={value.include.join('\n')}
        onChange={(event) => set({ include: lines(event.target.value) })}
      />
      <label className="checkbox-label">
        <input
          type="checkbox"
          checked={value.lights_must_say_light}
          onChange={(event) => set({ lights_must_say_light: event.target.checked })}
        />{' '}
        Lights must say LIGHT
      </label>
      <label className="checkbox-label">
        <input
          type="checkbox"
          checked={value.require_object}
          onChange={(event) => set({ require_object: event.target.checked })}
        />{' '}
        Lights need a target name
      </label>
      <label
        className="checkbox-label"
        title="Attach frames within 0.5° of a target whose name differs"
      >
        <input
          type="checkbox"
          checked={value.match_by_position}
          onChange={(event) => set({ match_by_position: event.target.checked })}
        />{' '}
        Match targets by position
      </label>
      <label htmlFor="scan-rules-min-exposure">Shortest light (s):</label>
      <input
        id="scan-rules-min-exposure"
        className="file-path-input scan-rules-exposure"
        type="number"
        min={0}
        step="any"
        placeholder="Any"
        value={value.min_exposure_seconds ?? ''}
        onChange={(event) =>
          set({
            min_exposure_seconds:
              event.target.value === '' ? null : Number(event.target.value),
          })
        }
      />
    </fieldset>
  );
}
