import type { StackColorCalibration } from '../api/types';
import { defaultColorCalibration } from './stackColorProcessing';

/** The Color calibration switch of a color composition or a one-shot-color
 *  stack's processing. A request without settings is calibrated with the
 *  defaults, so the switch reads on until it is turned off. */
export default function ColorCalibrationSwitch({
  calibration, disabled, onChange,
}: {
  calibration: StackColorCalibration | null | undefined;
  disabled: boolean;
  onChange: (calibration: StackColorCalibration) => void;
}) {
  const enabled = calibration?.enabled ?? true;
  return (
    <section className="stack-background-controls" aria-label="Color calibration">
      <header>
        <label>
          <input
            type="checkbox"
            role="switch"
            checked={enabled}
            disabled={disabled}
            onChange={(event) => onChange({
              ...(calibration ?? defaultColorCalibration()),
              enabled: event.target.checked,
            })}
          />
          <strong>Color calibration</strong>
        </label>
        <span>{enabled ? 'Gaia star colors' : 'Off'}</span>
      </header>
    </section>
  );
}
