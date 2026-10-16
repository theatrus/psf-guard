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
 *  `cutoff` stays the score as a fraction, as links have always held it. */
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
  const low = Math.min(...values);
  const high = Math.max(...values);
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
 * The cutoff when the URL names none. The score's is 50. For stars and HFR
 * it is where the quality score's own star and HFR terms reach zero at the
 * calibrated penalty: 40% fewer stars than the session's good frames (its
 * 90th percentile), or an HFR 30% over its sharp ones (its 10th), the
 * references `sequence_analysis.rs` scores against.
 */
export const DEFAULT_SCORE_CUTOFF = 50;
const STAR_DROP_CUTOFF = 0.6;
const HFR_RISE_CUTOFF = 1.3;

export function defaultCutoff(metric: CutoffMetric, values: readonly number[]): number | null {
  if (metric === 'score') return DEFAULT_SCORE_CUTOFF;
  if (values.length === 0) return null;
  return metric === 'stars'
    ? Math.round(percentile(values, 0.9) * STAR_DROP_CUTOFF)
    : Math.round(percentile(values, 0.1) * HFR_RISE_CUTOFF * 100) / 100;
}

/** A cutoff in the measure's units as the URL keeps it, or null for none.
 *  The score reads its fraction and answers 0 to 100. */
export function parseCutoff(metric: CutoffMetric, raw: string | null | undefined): number | null {
  if (raw === null || raw === undefined || raw.trim() === '') return null;
  const value = Number(raw);
  if (!Number.isFinite(value) || value < 0) return null;
  return metric === 'score' ? Math.min(100, value * 100) : value;
}

/** A cutoff as the URL keeps it: the score as a fraction. */
export function serializeCutoff(metric: CutoffMetric, value: number): string {
  if (metric === 'score') return (Math.round(value) / 100).toFixed(2);
  return metric === 'hfr' ? (Math.round(value * 100) / 100).toFixed(2) : String(Math.round(value));
}

export function formatCutoff(metric: CutoffMetric, value: number): string {
  return metric === 'hfr' ? value.toFixed(2) : String(Math.round(value));
}
