import type {
  CalibrationFinding,
  CalibrationFrameSummary,
  CalibrationKind,
  CalibrationSession,
  CalibrationValidDirection,
} from '../../api/types';

export const KIND_LABELS: Record<CalibrationKind, string> = {
  bias: 'Bias',
  dark: 'Dark',
  dark_flat: 'Dark-flat',
  flat: 'Flat',
};

export const KIND_PLURALS: Record<CalibrationKind, string> = {
  bias: 'Bias',
  dark: 'Darks',
  dark_flat: 'Dark-flats',
  flat: 'Flats',
};

/** The main finding of a grade, in a word or two. */
const GRADE_REASONS: Record<string, string> = {
  clipped: 'clipped',
  bright: 'too bright',
  dim: 'too dim',
  illumination: 'uneven light',
  features: 'spots',
  changing_features: 'changing spots',
  carried_features: 'spots',
  level: 'level',
  level_excess: 'stray light',
  noise: 'noise',
  row_banding: 'row banding',
  column_banding: 'column banding',
  hot_pixels: 'hot pixels',
};

export function gradeLabel(frame: Pick<CalibrationFrameSummary, 'grade' | 'grade_code'>): string | null {
  if (!frame.grade) return null;
  if (frame.grade === 'ok') return 'OK';
  const reason = frame.grade_code ? GRADE_REASONS[frame.grade_code] : undefined;
  const grade = frame.grade === 'poor' ? 'Poor' : 'Suspect';
  return reason ? `${grade} · ${reason}` : grade;
}

export const VALIDITY_LABELS: Record<CalibrationValidDirection, string> = {
  forward: '▸ after only',
  backward: '◂ before only',
};

export const VALIDITY_TITLES: Record<CalibrationValidDirection, string> = {
  forward: 'Serves only lights shot after it.',
  backward: 'Serves only lights shot before it.',
};

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

export function formatTime(timestamp?: number | null): string {
  if (timestamp === undefined || timestamp === null) return 'Time unknown';
  return new Date(timestamp * 1000).toLocaleString();
}

const seconds = (value: number) => `${Number(value.toFixed(2))}s`;

/** `2–2.4s`, or `-10.4 to -9.8 °C` where a dash would read as a minus. */
function range(low: number | null, high: number | null, digits: number, unit: string): string | null {
  if (low === null || high === null) return null;
  const text = (value: number) => String(Number(value.toFixed(digits)));
  if (low === high) return `${text(low)}${unit}`;
  return low < 0 ? `${text(low)} to ${text(high)}${unit}` : `${text(low)}–${text(high)}${unit}`;
}

/** The capture settings of one frame, in one line. */
export function formatSettings(frame: CalibrationFrameSummary): string {
  const values = [
    frame.width && frame.height ? `${frame.width}×${frame.height}` : null,
    frame.binning_x ? `${frame.binning_x}×${frame.binning_y ?? frame.binning_x} bin` : null,
    frame.gain !== null && frame.gain !== undefined ? `gain ${frame.gain}` : null,
    frame.offset !== null && frame.offset !== undefined ? `offset ${frame.offset}` : null,
    frame.readout_mode_name
      ? frame.readout_mode_name
      : frame.readout_mode !== null && frame.readout_mode !== undefined
        ? `readout ${frame.readout_mode}`
        : null,
    frame.exposure_s !== null && frame.exposure_s !== undefined ? seconds(frame.exposure_s) : null,
    frame.camera_temp !== null && frame.camera_temp !== undefined ? `${frame.camera_temp} °C` : null,
    frame.filter ? `filter ${frame.filter}` : null,
    frame.rotation !== null && frame.rotation !== undefined ? `${Number(frame.rotation.toFixed(1))}°` : null,
  ];
  return values.filter(Boolean).join(' · ') || 'No settings in the header';
}

/** What sets a session apart, in one short line. */
export function sessionTitle(session: CalibrationSession): string {
  const parts = [
    KIND_PLURALS[session.kind],
    session.filter && session.kind === 'flat' ? session.filter : null,
    session.kind !== 'flat' ? range(session.exposure_min_s, session.exposure_max_s, 2, 's') : null,
  ];
  return parts.filter(Boolean).join(' · ');
}

/** The rest of a session's settings. */
export function sessionDetails(session: CalibrationSession): string {
  const parts = [
    session.kind === 'flat' ? range(session.exposure_min_s, session.exposure_max_s, 2, 's') : null,
    range(session.temp_min_c, session.temp_max_c, 1, ' °C'),
    session.gain !== null ? `gain ${session.gain}` : null,
    session.binning && session.binning !== '1x1' ? `bin ${session.binning}` : null,
    session.rotation !== null && session.kind === 'flat' ? `${Number(session.rotation.toFixed(1))}°` : null,
    session.is_master ? 'master' : null,
  ];
  return parts.filter(Boolean).join(' · ');
}

export const nightLabel = (night: string | null | undefined) => night ?? 'Date unknown';

export const plural = (count: number, word: string, many = `${word}s`) =>
  `${count.toLocaleString()} ${count === 1 ? word : many}`;

/** The panel's label and unit for each measured number. */
const METRICS: Record<string, { label: string; format: (value: number) => string }> = {
  level: { label: 'Level', format: (value) => `${adu(value)} ADU` },
  sigma: { label: 'Spread', format: (value) => `${adu(value)} ADU` },
  noise: { label: 'Noise', format: (value) => `${value.toFixed(2)} ADU` },
  row_banding: { label: 'Row banding', format: (value) => `${value.toFixed(2)} ADU` },
  column_banding: { label: 'Column banding', format: (value) => `${value.toFixed(2)} ADU` },
  hot_pixels: { label: 'Hot pixels', format: (value) => Math.round(value).toLocaleString() },
  level_fraction: { label: 'Level', format: (value) => `${(value * 100).toFixed(1)}%` },
  clipped_fraction: { label: 'Clipped', format: (value) => `${(value * 100).toFixed(3)}%` },
  noise_fraction: { label: 'Noise', format: (value) => `${(value * 100).toFixed(2)}%` },
  features: { label: 'Spots', format: (value) => Math.round(value).toLocaleString() },
};

function adu(value: number): string {
  return Math.abs(value) < 10 ? value.toFixed(1) : Math.round(value).toLocaleString();
}

export function metricLabel(key: string): string {
  return METRICS[key]?.label ?? key;
}

export function formatMetric(key: string, value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—';
  return METRICS[key]?.format(value) ?? String(value);
}

/** A finding's limit in the unit its value has. */
export function formatLimit(finding: CalibrationFinding): string {
  const { code, limit } = finding;
  switch (code) {
    case 'clipped':
      return `${(limit * 100).toFixed(2)}%`;
    case 'bright':
    case 'dim':
    case 'illumination':
      return `${(limit * 100).toFixed(1)}%`;
    case 'features':
    case 'changing_features':
    case 'carried_features':
      return Math.round(limit).toLocaleString();
    default:
      return `${Number(limit.toFixed(2))}×`;
  }
}
