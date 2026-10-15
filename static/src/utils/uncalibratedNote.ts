import type { ExportSummary } from '../api/types';

/**
 * What an export says about the lights it left out because they can't be
 * calibrated, as stacks leave them out: how many, and one with its reason.
 * `null` when it left none out.
 */
export function uncalibratedNote(
  summary: Pick<ExportSummary, 'uncalibrated' | 'uncalibrated_example'>
): string | null {
  const count = summary.uncalibrated ?? 0;
  if (count === 0) return null;
  const example = summary.uncalibrated_example ? ` (${summary.uncalibrated_example})` : '';
  return `${count} light${count === 1 ? '' : 's'} that can't be calibrated left out${example}`;
}

/**
 * What an export says about the lights whose flats are from across a
 * measured change, which WBPP may not calibrate as a PSF Guard stack would:
 * how many, and one with why. `null` when there are none.
 */
export function flatWarningNote(
  summary: Pick<ExportSummary, 'flat_warnings' | 'flat_warning_example'>
): string | null {
  const count = summary.flat_warnings ?? 0;
  if (count === 0) return null;
  const example = summary.flat_warning_example ? ` (${summary.flat_warning_example})` : '';
  return `${count} light${count === 1 ? '' : 's'} with flats from across a measured change${example}`;
}
