import type { StackColorCalibrationOutcome, StackColorCalibrationSkipReason } from '../api/types';
import { openSettings } from '../utils/settingsIntent';

const skipText: Record<Exclude<StackColorCalibrationSkipReason, 'failed'>, string> = {
  no_plate_solve: 'reference frame not plate solved',
  no_gaia_catalog: 'Gaia colors not installed',
  too_few_stars: 'too few Gaia stars',
};

/** What color calibration did for one composition, in one line. */
export default function StackColorCalibrationLine({
  outcome,
  canInstall = true,
}: {
  outcome: StackColorCalibrationOutcome;
  /** Viewers cannot install catalogs, so they get no Install button. */
  canInstall?: boolean;
}) {
  if (outcome.status === 'calibrated') {
    const [red, , blue] = outcome.gains;
    return (
      <div
        className="stack-color-calibration"
        data-calibration={outcome.stretched_separately ? 'separate' : 'calibrated'}
        title={outcome.stretched_separately
          ? 'Each channel has its own stretch, which undoes most of the balance. Give them the same stretch to keep it.'
          : undefined}
      >
        Calibrated on {outcome.stars_used} stars · R ×{red.toFixed(2)}, B ×{blue.toFixed(2)}
        {outcome.stretched_separately && ' · channels stretched separately'}
      </div>
    );
  }
  const reason = outcome.reason === 'failed' ? outcome.detail : skipText[outcome.reason];
  return (
    <div
      className="stack-color-calibration"
      data-calibration="skipped"
      title={outcome.reason === 'failed' ? undefined : outcome.detail}
    >
      <span>Not color calibrated: {reason}</span>
      {outcome.reason === 'no_gaia_catalog' && canInstall && (
        <button
          type="button"
          onClick={() => openSettings('catalogs', { field: 'gaia_photometry' })}
        >
          Install
        </button>
      )}
    </div>
  );
}
