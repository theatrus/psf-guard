import { describe, expect, it } from 'vitest';
import type { Image, ImageQualityResult } from '../../api/types';
import {
  cutoffScale,
  defaultCutoff,
  metricValue,
  parseCutoff,
  parseCutoffMetric,
  pastCutoff,
  serializeCutoff,
} from '../sequenceCutoff';

const quality = (quality_score: number) => ({ image_id: 1, quality_score }) as ImageQualityResult;
const image = (metadata: Record<string, unknown>) => ({ id: 1, metadata }) as unknown as Image;

describe('sequence cutoff', () => {
  it('reads the measure from the URL, the score by default', () => {
    expect(parseCutoffMetric(null)).toBe('score');
    expect(parseCutoffMetric('stars')).toBe('stars');
    expect(parseCutoffMetric('hfr')).toBe('hfr');
    expect(parseCutoffMetric('fwhm')).toBe('score');
  });

  it('shows the score 0 to 100 and keeps it in the URL as a fraction', () => {
    expect(parseCutoff('score', '0.65')).toBe(65);
    expect(parseCutoff('score', '3')).toBe(100);
    expect(parseCutoff('score', 'soon')).toBeNull();
    expect(parseCutoff('score', null)).toBeNull();
    expect(serializeCutoff('score', 72)).toBe('0.72');
    expect(parseCutoff('hfr', '2.40')).toBe(2.4);
    expect(serializeCutoff('hfr', 2.456)).toBe('2.46');
    expect(serializeCutoff('stars', 812.4)).toBe('812');
  });

  it('reads each frame in the units its chip shows', () => {
    expect(metricValue('score', quality(0.734), undefined)).toBeCloseTo(73.4);
    expect(metricValue('stars', quality(1), image({ DetectedStars: 950 }))).toBe(950);
    expect(metricValue('hfr', quality(1), image({ HFR: 2.31 }))).toBe(2.31);
    expect(metricValue('hfr', quality(1), image({ HFR: 0 }))).toBeNull();
    expect(metricValue('stars', quality(1), image({}))).toBeNull();
  });

  it('marks fewer stars and a lower score below, a larger HFR above', () => {
    expect(pastCutoff('score', 40, 50)).toBe(true);
    expect(pastCutoff('stars', 400, 500)).toBe(true);
    expect(pastCutoff('stars', 600, 500)).toBe(false);
    expect(pastCutoff('hfr', 3.1, 3)).toBe(true);
    expect(pastCutoff('hfr', 2.9, 3)).toBe(false);
    expect(pastCutoff('hfr', null, 3)).toBe(false);
  });

  it('slides over 0 to 100 for the score and the readings on screen otherwise', () => {
    expect(cutoffScale('score', [])).toEqual({ min: 0, max: 100, step: 1 });
    expect(cutoffScale('stars', [812, 1200.4, 990])).toEqual({ min: 812, max: 1201, step: 1 });
    expect(cutoffScale('hfr', [2.314, 2.871])).toEqual({ min: 2.31, max: 2.88, step: 0.01 });
    expect(cutoffScale('hfr', [])).toBeNull();
    // All alike: the thumb still has somewhere to go.
    expect(cutoffScale('stars', [900, 900])).toEqual({ min: 899, max: 901, step: 1 });
  });

  it('defaults to where the score takes its full star and HFR penalty', () => {
    expect(defaultCutoff('score', [])).toBe(50);
    // 40% under the 90th percentile, 1080.
    expect(defaultCutoff('stars', [900, 1000, 1100])).toBe(648);
    // 30% over the 10th percentile, 2.04.
    expect(defaultCutoff('hfr', [2, 2.2, 2.4])).toBe(2.65);
    expect(defaultCutoff('hfr', [])).toBeNull();
  });
});
