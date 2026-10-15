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
