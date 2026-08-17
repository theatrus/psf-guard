import type { WcsSolution } from '@seiza/astro-overlay';

export interface FieldRotation {
  /** Direction of celestial north measured from image "up", in degrees,
   * positive toward image right, normalized to (−180, 180]. */
  degrees: number;
  /** True when the field is mirrored (east and west swap sides compared
   * with the sky) — a flat, mirror diagonal, or some OAG paths. */
  mirrored: boolean;
}

/**
 * Camera field rotation from a WCS solve's CD matrix.
 *
 * The CD matrix maps pixel offsets to sky offsets; inverting its second
 * column gives the pixel direction of +Dec (north). The angle of that
 * vector from screen-up is the field rotation. Parity (the determinant
 * sign) says whether the frame is mirrored.
 */
export function fieldRotation(wcs: WcsSolution): FieldRotation | null {
  const [[cd11, cd12], [cd21, cd22]] = wcs.cd;
  const det = cd11 * cd22 - cd12 * cd21;
  if (!Number.isFinite(det) || det === 0) return null;
  // inv(CD) · (0, 1): pixel displacement per degree of declination.
  const northX = -cd12 / det;
  const northY = cd11 / det;
  if (northX === 0 && northY === 0) return null;
  // Screen up is −y; positive angles rotate toward +x (image right).
  const degrees = (Math.atan2(northX, -northY) * 180) / Math.PI;
  // Sky convention: with north up, east is to the LEFT. Working the east
  // vector inv(CD)·(1,0) through image coordinates (y down), that parity
  // corresponds to a positive determinant; negative means mirrored.
  return { degrees, mirrored: det < 0 };
}
