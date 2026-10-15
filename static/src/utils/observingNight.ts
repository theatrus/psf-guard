/**
 * Where a catalog's nights split when the server cannot say: noon UTC, the
 * server's own fallback for a catalog whose lights it cannot read.
 */
export const FALLBACK_NIGHT_BOUNDARY_SECONDS = 12 * 3600;

/**
 * The observing night a capture belongs to, `YYYY-MM-DD`, for a catalog whose
 * nights split `boundarySeconds` after 00:00 UTC: the date the evening began,
 * so an evening and the small hours after it share one night. The server's
 * `sky_coverage::night_of` counts nights the same way, as calibration and the
 * AstroBin export do, and `/stats/overall` reports the catalog's boundary.
 */
export function nightOf(capturedAt: number, boundarySeconds: number): string {
  const instant = new Date((capturedAt - boundarySeconds) * 1000);
  return Number.isNaN(instant.getTime()) ? 'Unknown Date' : instant.toISOString().slice(0, 10);
}
