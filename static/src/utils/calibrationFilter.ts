import type { CalibrationHealthCode, CalibrationHealthLevel, CalibrationLightMark } from '../api/types';

/**
 * The Images tab's Calibration filter: lights the calibration library
 * serves poorly. Each value is a level, `poor` or `check`, which a light's
 * worst finding meets, or a finding's code (`cannot_calibrate`), which the
 * coverage report's links carry. The URL key is `calib`.
 */
export const CALIBRATION_LEVELS: ReadonlyArray<{ value: CalibrationHealthLevel; label: string }> = [
  { value: 'poor', label: 'Poor' },
  { value: 'check', label: 'Check' },
];

const CODE_LABELS: Record<CalibrationHealthCode, string> = {
  no_library: 'No library match',
  cannot_calibrate: "Can't be calibrated",
  thin_dark: 'Thin dark master',
  thin_bias: 'Thin bias master',
  flat_spots: 'Spots in flats',
  old_flats: 'Old flats',
  mixed_flats: 'Flats from several nights',
  suspect_frames: 'Suspect frames',
  short_after_keep_out: 'Short after keep-out',
};

export function calibrationFilterLabel(value: string): string {
  return CALIBRATION_LEVELS.find((level) => level.value === value)?.label
    ?? CODE_LABELS[value as CalibrationHealthCode]
    ?? value;
}

/** The few words a mark's codes come to, for a chip's title. */
export function calibrationMarkTitle(mark: CalibrationLightMark): string {
  const head = mark.level === 'poor' ? "Can't be calibrated properly" : 'Calibration worth a check';
  return `${head}: ${mark.codes.map((code) => CODE_LABELS[code] ?? code).join(', ')}`;
}

/** A light passes when nothing is chosen, or when its mark meets any
 *  chosen level or carries any chosen code. A light with no mark passes
 *  only an empty filter. */
export function matchesCalibrationFilter(
  chosen: readonly string[],
  mark: CalibrationLightMark | undefined,
): boolean {
  if (chosen.length === 0) return true;
  if (!mark) return false;
  return chosen.some((value) => value === mark.level || mark.codes.includes(value as CalibrationHealthCode));
}
