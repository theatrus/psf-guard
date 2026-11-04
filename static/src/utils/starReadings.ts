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
  /** The source the frame's whole set is compared on, which the Sequence
   *  cutoff reads so no frame is ranked against another source's values. */
  compared: StarSource | null;
}

export type StarMetric = 'stars' | 'hfr';

/** Each source's mark as a superscript letter, for plain text such as a
 *  summary line: N for N.I.N.A., P for PSF Guard. */
export const SOURCE_SUPERSCRIPTS: Record<StarSource, string> = { capture: 'ᴺ', scan: 'ᴾ' };

const finite = (value: unknown): number | null =>
  typeof value === 'number' && Number.isFinite(value) ? value : null;

/** An HFR of zero is no measurement, not an impossibly sharp one. */
const reading = (metric: StarMetric, value: unknown): number | null => {
  const number = finite(value);
  return metric === 'hfr' && number !== null && number <= 0 ? null : number;
};

/** Which metadata fields PSF Guard's scan wrote into an imported frame's
 *  metadata. A source recorded without its field list owns both, as in
 *  early write-backs. The same rule as `extract_metrics_from_metadata` in
 *  src/sequence_analysis.rs. */
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
      compared: stars.compared_on[key],
    };
  }
  const none: StarReadings = { capture: null, scan: null, used: null, compared: null };
  const metadata = image?.metadata as Record<string, unknown> | undefined;
  const field = metric === 'stars' ? 'DetectedStars' : 'HFR';
  const value = reading(metric, metadata?.[field]);
  if (value === null) return none;
  const scanWrote = writtenBack(metadata);
  if (!scanWrote.includes(field)) return { ...none, capture: value, used: 'capture', compared: 'capture' };
  // The scan's HFR beside N.I.N.A.'s count would mix scales in one frame;
  // the server leaves such an HFR out, and so does this.
  if (metric === 'hfr' && !scanWrote.includes('DetectedStars')) return none;
  return { ...none, scan: value, used: 'scan', compared: 'scan' };
}

/** The reading scoring judged the frame on alone, or null when it has none. */
export function usedReading(readings: StarReadings): number | null {
  return readings.used === null ? null : readings[readings.used];
}

/** The reading the frame's set is compared on, or null when the frame has
 *  none from that source. */
export function comparedReading(readings: StarReadings): number | null {
  return readings.compared === null ? null : readings[readings.compared];
}

/** A reading as the views show it: HFR to two places, stars whole. */
export function formatReading(metric: StarMetric, value: number): string {
  return metric === 'hfr' ? value.toFixed(2) : String(Math.round(value));
}
