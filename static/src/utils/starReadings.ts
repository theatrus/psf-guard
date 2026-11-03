import type { Image, ImageQualityResult, StarSource } from '../api/types';

/**
 * A frame's star count or HFR from each source: what N.I.N.A. recorded
 * during capture and what PSF Guard's quality scan measured. The two use
 * different detectors, so they differ; scoring uses one, the Scoring
 * control's choice when the frame has it, and `used` is that one.
 */
export interface StarReadings {
  capture: number | null;
  scan: number | null;
  used: StarSource | null;
}

export type StarMetric = 'stars' | 'hfr';

const finite = (value: unknown): number | null =>
  typeof value === 'number' && Number.isFinite(value) ? value : null;

/** An HFR of zero is no measurement, not an impossibly sharp one. */
const reading = (metric: StarMetric, value: unknown): number | null => {
  const number = finite(value);
  return metric === 'hfr' && number !== null && number <= 0 ? null : number;
};

/** Which metadata fields PSF Guard's scan wrote into an imported frame's
 *  metadata. A source recorded without its field list owns both, as in
 *  early write-backs. */
function writtenBack(metadata: Record<string, unknown> | undefined): string[] {
  const fields = metadata?.PsfGuardQualityFields;
  if (Array.isArray(fields)) return fields.filter((field): field is string => typeof field === 'string');
  return typeof metadata?.PsfGuardQualitySource === 'string' ? ['DetectedStars', 'HFR'] : [];
}

/** The readings scoring returned, or, before scoring has, the one the
 *  catalog's metadata holds. */
export function starReadings(
  metric: StarMetric,
  quality: ImageQualityResult | null | undefined,
  image: Image | null | undefined,
): StarReadings {
  const stars = quality?.stars;
  if (stars) {
    const key = metric === 'stars' ? 'star_count' : 'hfr';
    return {
      capture: reading(metric, stars.capture?.[key]),
      scan: reading(metric, stars.scan?.[key]),
      used: (metric === 'stars' ? stars.star_count_source : stars.hfr_source) ?? null,
    };
  }
  const metadata = image?.metadata as Record<string, unknown> | undefined;
  const field = metric === 'stars' ? 'DetectedStars' : 'HFR';
  const value = reading(metric, metadata?.[field]);
  if (value === null) return { capture: null, scan: null, used: null };
  return writtenBack(metadata).includes(field)
    ? { capture: null, scan: value, used: 'scan' }
    : { capture: value, scan: null, used: 'capture' };
}

/** The reading scoring used, or null when the frame has none. */
export function usedReading(readings: StarReadings): number | null {
  if (readings.used === null) return null;
  return readings[readings.used];
}

/** A reading as the views show it: HFR to two places, stars whole. */
export function formatReading(metric: StarMetric, value: number): string {
  return metric === 'hfr' ? value.toFixed(2) : String(Math.round(value));
}
