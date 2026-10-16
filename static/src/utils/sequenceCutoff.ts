import type { Image, ImageQualityResult } from '../api/types';

/**
 * What the Sequence view's cutoff measures. Score is the quality score as
 * the chips show it, 0 to 100; Stars and HFR are N.I.N.A.'s detected star
 * count and half-flux radius, as the cards show them. The chart, the slider
 * and the Below the cutoff rule all follow the one chosen.
 */
export type CutoffMetric = 'score' | 'stars' | 'hfr';

export interface CutoffMetricInfo {
  id: CutoffMetric;
  label: string;
  /** Which side of the cutoff a poor frame falls on: fewer stars and a
   *  lower score are worse, a larger HFR is worse. */
  worse: 'below' | 'above';
  /** The Below the cutoff rule's name in this measure. */
  ruleLabel: string;
  ruleDescription: string;
}

export const CUTOFF_METRICS: readonly CutoffMetricInfo[] = [
  {
    id: 'score',
    label: 'Score',
    worse: 'below',
    ruleLabel: 'Below the cutoff',
    ruleDescription: 'Frames scoring under the score cutoff in the view on screen',
  },
  {
    id: 'stars',
    label: 'Stars',
    worse: 'below',
    ruleLabel: 'Fewer stars than the cutoff',
    ruleDescription: 'Frames detecting fewer stars than the star cutoff in the view on screen',
  },
  {
    id: 'hfr',
    label: 'HFR',
    worse: 'above',
    ruleLabel: 'HFR above the cutoff',
    ruleDescription: 'Frames whose HFR is above the HFR cutoff in the view on screen',
  },
];

export const DEFAULT_CUTOFF_METRIC: CutoffMetric = 'score';

/** The URL keys: `cutoffBy` names the measure (score leaves it off), and
 *  each measure keeps its own cutoff, so switching back finds it again.
 *  `cutoff` stays the score as a fraction, as links have always held it;
 *  `starCutoff` and `hfrCutoff` are ratios to the reference (see
 *  `resolveCutoff`), such as 0.6 for 40% fewer stars than the best frames. */
export const CUTOFF_METRIC_PARAM = 'cutoffBy';
export const CUTOFF_PARAMS: Record<CutoffMetric, string> = {
  score: 'cutoff',
  stars: 'starCutoff',
  hfr: 'hfrCutoff',
};

export function parseCutoffMetric(raw: string | null | undefined): CutoffMetric {
  return CUTOFF_METRICS.find((metric) => metric.id === raw)?.id ?? DEFAULT_CUTOFF_METRIC;
}

export function cutoffMetricOf(id: CutoffMetric): CutoffMetricInfo {
  return CUTOFF_METRICS.find((metric) => metric.id === id)!;
}

/** A frame's reading in a measure, in the units its chip shows, or null
 *  when the frame has none. */
export function metricValue(
  metric: CutoffMetric,
  quality: ImageQualityResult,
  image: Image | undefined,
): number | null {
  switch (metric) {
    case 'score':
      return quality.quality_score * 100;
    case 'stars': {
      const stars = image?.metadata?.DetectedStars;
      return typeof stars === 'number' && Number.isFinite(stars) ? stars : null;
    }
    case 'hfr': {
      const hfr = image?.metadata?.HFR;
      return typeof hfr === 'number' && Number.isFinite(hfr) && hfr > 0 ? hfr : null;
    }
  }
}

/** Whether a reading falls on the poor side of the cutoff. */
export function pastCutoff(metric: CutoffMetric, value: number | null, cutoff: number): boolean {
  if (value === null) return false;
  return cutoffMetricOf(metric).worse === 'below' ? value < cutoff : value > cutoff;
}

/** The slider's travel: 0 to 100 for the score, the readings the session
 *  on screen holds for stars and HFR. */
export interface CutoffScale {
  min: number;
  max: number;
  step: number;
}

export function cutoffScale(metric: CutoffMetric, values: readonly number[]): CutoffScale | null {
  if (metric === 'score') return { min: 0, max: 100, step: 1 };
  if (values.length === 0) return null;
  const step = metric === 'hfr' ? 0.01 : 1;
  // A loop, not Math.min(...values): a long all-sessions view would pass
  // more arguments than an engine takes.
  let low = Infinity;
  let high = -Infinity;
  for (const value of values) {
    if (value < low) low = value;
    if (value > high) high = value;
  }
  const min = metric === 'hfr' ? Math.floor(low * 100) / 100 : Math.floor(low);
  const max = metric === 'hfr' ? Math.ceil(high * 100) / 100 : Math.ceil(high);
  // One reading, or all alike: leave the thumb somewhere to go.
  return max > min ? { min, max, step } : { min: Math.max(0, min - step), max: max + step, step };
}

/** A linearly interpolated quantile, as the server's `percentile`. */
function percentile(values: readonly number[], quantile: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  const position = quantile * (sorted.length - 1);
  const lower = Math.floor(position);
  const upper = Math.ceil(position);
  return sorted[lower] + (sorted[upper] - sorted[lower]) * (position - lower);
}

/**
 * What a Stars or HFR cutoff is kept relative to: the reading the quality
 * score measures the frames on screen against in `sequence_analysis.rs`,
 * their 90th-percentile star count or their 10th-percentile HFR. Null for
 * the score, and when the frames give no reference to divide by.
 */
export function cutoffReference(metric: CutoffMetric, values: readonly number[]): number | null {
  if (metric === 'score' || values.length === 0) return null;
  const reference = percentile(values, metric === 'stars' ? 0.9 : 0.1);
  return reference > 0 ? reference : null;
}

/** The HFR-rise and star-loss penalties the view scores with (see
 *  utils/scoring.ts): they move where the score's own term reaches zero. */
export interface CutoffPenalties {
  hfrRise: number;
  starLoss: number;
}

/** Where the score's star and HFR terms reach zero at the calibrated
 *  penalty: 40% fewer stars, or 30% more HFR, than the reference. The
 *  server's STAR_COUNT_TOLERANCE and HFR_TOLERANCE. */
const STAR_ZERO_SCORE_DROP = 0.4;
const HFR_ZERO_SCORE_RISE = 0.3;

export const DEFAULT_SCORE_CUTOFF = 50;

/**
 * The default Stars or HFR cutoff, as a ratio to the reference: where the
 * score's own term reaches zero under the view's penalty, which divides
 * the band. At 0% that term never reaches zero, and the cutoff sits past
 * every reading.
 */
function defaultCutoffRatio(metric: 'stars' | 'hfr', penalties: CutoffPenalties): number {
  if (metric === 'stars') {
    return penalties.starLoss > 0 ? 1 - STAR_ZERO_SCORE_DROP / penalties.starLoss : -Infinity;
  }
  return penalties.hfrRise > 0 ? 1 + HFR_ZERO_SCORE_RISE / penalties.hfrRise : Infinity;
}

/** A cutoff on the slider's own steps: HFR to hundredths, the rest whole. */
function onSlider(metric: CutoffMetric, value: number): number {
  return metric === 'hfr' ? Math.round(value * 100) / 100 : Math.round(value);
}

export interface ResolvedCutoff {
  /** The slider's travel, or null while no frame on screen has a reading. */
  scale: CutoffScale | null;
  /** The cutoff in the measure's units, on the slider. */
  value: number | null;
}

/**
 * The cutoff the view shows. `stored` is what the URL holds (see
 * `parseCutoff`): the score 0 to 100, or a Stars or HFR ratio to the
 * reference, so one cutoff means the same on every session tab and
 * filter, whose readings differ. Null takes the default.
 */
export function resolveCutoff(
  metric: CutoffMetric,
  stored: number | null,
  values: readonly number[],
  penalties: CutoffPenalties,
): ResolvedCutoff {
  const scale = cutoffScale(metric, values);
  if (!scale) return { scale: null, value: null };
  const clamp = (value: number) => Math.min(scale.max, Math.max(scale.min, value));
  if (metric === 'score') return { scale, value: clamp(onSlider(metric, stored ?? DEFAULT_SCORE_CUTOFF)) };
  const reference = cutoffReference(metric, values);
  if (reference === null) return { scale, value: null };
  const ratio = stored ?? defaultCutoffRatio(metric, penalties);
  return { scale, value: clamp(onSlider(metric, ratio * reference)) };
}

/** What the URL keeps for a cutoff the slider set: null at the default,
 *  so a link stays clean and every tab takes its own default. */
export function storedCutoff(
  metric: CutoffMetric,
  value: number,
  values: readonly number[],
  penalties: CutoffPenalties,
): number | null {
  const shown = onSlider(metric, value);
  if (shown === resolveCutoff(metric, null, values, penalties).value) return null;
  if (metric === 'score') return shown;
  const reference = cutoffReference(metric, values);
  return reference === null ? null : shown / reference;
}

/** A cutoff as the URL keeps it, or null for none: the score read from
 *  its fraction as 0 to 100, Stars and HFR as a ratio to the reference. */
export function parseCutoff(metric: CutoffMetric, raw: string | null | undefined): number | null {
  if (raw === null || raw === undefined || raw.trim() === '') return null;
  const value = Number(raw);
  if (!Number.isFinite(value) || value < 0) return null;
  if (metric === 'score') return Math.min(100, value * 100);
  return value > 0 ? value : null;
}

/** A cutoff as the URL keeps it: the score as a fraction, as links have
 *  always held it, and a Stars or HFR ratio to four places. */
export function serializeCutoff(metric: CutoffMetric, value: number): string {
  if (metric === 'score') return (Math.round(value) / 100).toFixed(2);
  return String(Math.round(value * 10_000) / 10_000);
}

export function formatCutoff(metric: CutoffMetric, value: number): string {
  return metric === 'hfr' ? value.toFixed(2) : String(Math.round(value));
}
