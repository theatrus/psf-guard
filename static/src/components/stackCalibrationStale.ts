import type { StackStretchPreview } from '../api/types';

/** Why a one-shot-color stack's calibrated view is out of date, as the
 *  server worked it out: applied processing speaks for itself, and without
 *  any the build's card preview does. Mono stacks never are. */
export function calibrationOutdated(
  group: { output_channels: number; color_calibration_outdated?: string | null },
  applied: Pick<StackStretchPreview, 'color_calibration_outdated'> | null | undefined
): string | null {
  if (group.output_channels !== 3) return null;
  return (applied ? applied.color_calibration_outdated : group.color_calibration_outdated) ?? null;
}
