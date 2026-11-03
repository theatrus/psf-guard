import type { ProfileRecordView, StarNoiseReduction, StarSensitivity } from '../api/types';

const SENSITIVITY: Record<StarSensitivity, string> = {
  normal: 'Normal sensitivity',
  high: 'High sensitivity',
  highest: 'Highest sensitivity',
};

const NOISE_REDUCTION: Record<StarNoiseReduction, string> = {
  none: 'no noise reduction',
  normal: 'Normal noise reduction',
  high: 'High noise reduction',
  highest: 'Highest noise reduction',
  median: 'Median noise reduction',
};

const MODE: Record<ProfileRecordView['hocus_focus_mode'], string> = {
  simple: 'Simple',
  optimized: 'Optimized',
  advanced: 'Advanced',
};

/** A number as the profile holds it, without float noise: 1.2000000000000002 reads 1.2. */
function plain(value: number): string {
  return String(Number(value.toPrecision(6)));
}

/** The crop a record applies, as "crop 0.7" or "crop 0.5–0.9"; null for none. */
function crop(inner: number, outer: number): string | null {
  if (inner >= 1 && outer >= 1) return null;
  if (outer >= 1 || inner >= 1) return `crop ${plain(Math.min(inner, outer))}`;
  return `crop ${plain(inner)}–${plain(outer)}`;
}

/** The detector a record selects, by name. */
export function recordDetector(record: ProfileRecordView): string {
  const detector = record.settings.star_detector;
  if (detector === 'hocus_focus') return `Hocus Focus ${record.hocus_focus_version === 'v3' ? 3 : 4}`;
  if (detector === 'nina') return 'N.I.N.A.';
  return detector;
}

/** A record's values in one line: detector, mode, crop, brightest stars,
 *  and the Hocus Focus values that most change what it finds. */
export function recordSummary(record: ProfileRecordView): string {
  const settings = record.settings;
  const parts = [recordDetector(record)];
  if (settings.star_detector === 'hocus_focus') {
    const knobs = record.hocus_focus;
    parts.push(MODE[record.hocus_focus_mode]);
    const cropped = crop(knobs.autofocus_inner_crop_ratio, knobs.autofocus_outer_crop_ratio);
    if (cropped) parts.push(cropped);
    if (knobs.autofocus_brightest_stars > 0) parts.push(`brightest ${knobs.autofocus_brightest_stars}`);
    parts.push(
      `layers ${knobs.structure_layers}`,
      `sensitivity ${plain(knobs.brightness_sensitivity)}`,
      `min box ${knobs.min_star_bounding_box}`,
      `noise radius ${knobs.noise_reduction_radius}`,
      `sample ${plain(knobs.pixel_sample_size)}`,
    );
  } else {
    parts.push(SENSITIVITY[settings.star_sensitivity], NOISE_REDUCTION[settings.noise_reduction]);
    const cropped = crop(settings.autofocus_inner_crop_ratio, settings.autofocus_outer_crop_ratio);
    if (cropped) parts.push(cropped);
  }
  return parts.join(' · ');
}

function day(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleDateString(undefined, { dateStyle: 'medium' });
}

/** Which frames a record applies to: "All frames" or "From 9 Sep 2026". */
export function recordSpan(record: ProfileRecordView): string {
  return record.effective_from ? `From ${day(record.effective_from)}` : 'All frames';
}

const ORIGIN: Record<ProfileRecordView['source'], string> = {
  'plugin:sync': 'Sent by the Sync plugin',
  'plugin:director': 'Sent by the Director plugin',
  import: 'Imported',
};

/** Where a record came from and when: "Imported 2 Nov 2026". */
export function recordOrigin(record: ProfileRecordView): string {
  return `${ORIGIN[record.source] ?? 'Recorded'} ${day(record.received_at)}`;
}

/** The start of a local calendar day, as the server takes it. */
export function startOfLocalDay(date: string): string | undefined {
  const parsed = new Date(`${date}T00:00:00`);
  return Number.isNaN(parsed.getTime()) ? undefined : parsed.toISOString();
}
