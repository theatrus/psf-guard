import type {
  CalibrationKind,
  CalibrationKindExplanation,
  LightCalibrationFinding,
} from '../api/types';

/** The value of the detail's calibration focus that chooses no finding and
 *  shows every kind. (The URL helper drops a value of `all`.) */
export const NO_FINDING = 'none';

/** One part of a kind's account, which a finding can point at. */
export type CalibrationPart = 'master' | 'used' | 'unused' | 'suspect' | 'kept_out' | 'refused' | 'other';

/**
 * The finding the calibration panel shows: the one the detail chose, else
 * the first the Images calibration filter asks for (a code or a level),
 * else the worst. `null` when none is chosen or the light has none.
 */
export function chosenFinding(
  findings: readonly LightCalibrationFinding[],
  focus: string | null,
  gridFilter: readonly string[],
): LightCalibrationFinding | null {
  if (focus === NO_FINDING) return null;
  const byCode = (code: string) => findings.find((finding) => finding.code === code);
  const chosen = focus ? byCode(focus) : undefined;
  if (chosen) return chosen;
  for (const value of gridFilter) {
    const match = byCode(value) ?? findings.find((finding) => finding.level === value);
    if (match) return match;
  }
  return findings[0] ?? null;
}

/**
 * The finding a light's chip opens on: the first of its codes the Images
 * calibration filter asks for, else its first.
 */
export function chipFocus(codes: readonly string[], gridFilter: readonly string[]): string | undefined {
  return codes.find((code) => gridFilter.includes(code)) ?? codes[0];
}

/** What `finding` points at in one kind; empty when it is about another. */
export function focusParts(
  finding: LightCalibrationFinding | null,
  kind: CalibrationKindExplanation,
): ReadonlySet<CalibrationPart> {
  if (!finding || !(finding.kinds ?? []).includes(kind.kind)) return new Set();
  switch (finding.code) {
    case 'no_library':
      return new Set(['refused', 'other']);
    case 'cannot_calibrate':
      // Nothing matched: why each frame was refused. Flats too old: their nights.
      return new Set(kind.master.frames === 0 ? ['refused', 'other'] : ['used']);
    case 'thin_dark':
    case 'thin_bias':
      return new Set(['master', 'used', 'kept_out']);
    case 'flat_spots':
      return new Set(['master', 'suspect']);
    case 'old_flats':
    case 'mixed_flats':
      return new Set(['used']);
    case 'suspect_frames':
      return new Set(['suspect']);
    case 'short_after_keep_out':
      return new Set(['master', 'kept_out']);
    case 'flats_differ':
      // The flats it takes and those on the other side of the change.
      return new Set(['master', 'used', 'unused']);
  }
}

/** What a finding means for the light, in a sentence. */
export const FINDING_HINTS: Record<LightCalibrationFinding['code'], string> = {
  no_library: 'Nothing in the library was shot with this camera and these settings.',
  cannot_calibrate: 'A stack cannot calibrate this light properly with the library as it is.',
  thin_dark: 'A master from few darks adds its own noise to every light it calibrates.',
  thin_bias: 'A master from few bias frames adds its own noise to the flats and lights.',
  flat_spots: 'Spots that change between flats, such as drying dew, end up in the master.',
  old_flats: 'Dust moves between nights; flats shot long before or after may not match it.',
  mixed_flats: 'One master averages flats from nights not measured against each other, each with its own dust.',
  suspect_frames: 'The master takes frames whose grade says they are worth a look.',
  short_after_keep_out: 'Frames kept out by hand leave too few for a clean master.',
  flats_differ: 'A measured change, such as new dust, lies between this light and its flats.',
};

const KIND_LABELS: Record<CalibrationKind, string> = {
  bias: 'Bias',
  dark: 'Darks',
  dark_flat: 'Dark flats',
  flat: 'Flats',
};

export function calibrationKindLabel(kind: CalibrationKind): string {
  return KIND_LABELS[kind];
}

/** Whole days between two `YYYY-MM-DD` nights, or `null` if either is not one. */
export function nightsApart(left: string | null | undefined, right: string | null | undefined): number | null {
  const day = (night: string | null | undefined) => {
    if (!night || !/^\d{4}-\d{2}-\d{2}$/.test(night)) return null;
    const at = Date.parse(`${night}T00:00:00Z`);
    return Number.isFinite(at) ? at / 86_400_000 : null;
  };
  const a = day(left);
  const b = day(right);
  return a === null || b === null ? null : Math.round(Math.abs(a - b));
}

/** A temperature range, `−10.0 °C` or `−10.2 to −9.8 °C`. */
export function temperatureRange(min?: number | null, max?: number | null): string | null {
  const format = (value: number) => `${value < 0 ? '−' : ''}${Math.abs(value).toFixed(1)}`;
  if (min == null || max == null) return null;
  return min === max ? `${format(min)} °C` : `${format(min)} to ${format(max)} °C`;
}
