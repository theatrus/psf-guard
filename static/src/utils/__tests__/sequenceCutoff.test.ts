import { describe, expect, it } from 'vitest';
import type { Image, ImageQualityResult } from '../../api/types';
import {
  cutoffReference,
  cutoffScale,
  metricValue,
  parseCutoff,
  parseCutoffMetric,
  pastCutoff,
  resolveCutoff,
  serializeCutoff,
  storedCutoff,
} from '../sequenceCutoff';

const CALIBRATED = { hfrRise: 1, starLoss: 1 };

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
  });

  it('keeps a Stars or HFR cutoff as a ratio to the reference', () => {
    expect(parseCutoff('hfr', '1.25')).toBe(1.25);
    expect(parseCutoff('stars', '0')).toBeNull();
    expect(serializeCutoff('hfr', 1.267_94)).toBe('1.2679');
    expect(serializeCutoff('stars', 0.6)).toBe('0.6');
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

  it('measures stars against the 90th percentile and HFR against the 10th', () => {
    expect(cutoffReference('stars', [900, 1000, 1100])).toBe(1080);
    expect(cutoffReference('hfr', [2, 2.2, 2.4])).toBeCloseTo(2.04);
    expect(cutoffReference('score', [50])).toBeNull();
    expect(cutoffReference('stars', [0, 0])).toBeNull();
  });

  it('defaults to where the score takes its full star and HFR penalty', () => {
    expect(resolveCutoff('score', null, [], CALIBRATED).value).toBe(50);
    // 40% under the 90th percentile, 1070; 30% over the 10th, 2.06.
    expect(resolveCutoff('stars', null, [600, 900, 1000, 1100], CALIBRATED).value).toBe(642);
    expect(resolveCutoff('hfr', null, [2, 2.2, 2.4, 2.8], CALIBRATED).value).toBe(2.68);
    expect(resolveCutoff('hfr', null, [], CALIBRATED)).toEqual({ scale: null, value: null });
  });

  it('moves the default with the HFR-rise and star-loss penalties', () => {
    const hfrs = [2, 2.2, 2.4, 2.8];
    // 200% halves the band: zero at 15% over 2.06, 2.369.
    expect(resolveCutoff('hfr', null, hfrs, { hfrRise: 2, starLoss: 1 }).value).toBe(2.37);
    // 0% never takes the term to zero: past every reading.
    expect(resolveCutoff('hfr', null, hfrs, { hfrRise: 0, starLoss: 1 }).value).toBe(2.8);
    expect(resolveCutoff('stars', null, [600, 900, 1000, 1100], { hfrRise: 1, starLoss: 0 }).value).toBe(600);
  });

  it('means the same on another session whose readings differ', () => {
    const bright = [700, 800, 850, 900];
    const faint = [200, 240, 270, 300];
    const stored = storedCutoff('stars', 750, bright, CALIBRATED)!;
    // 750 of the bright session's 885 reference.
    expect(stored).toBeCloseTo(750 / 885);
    expect(resolveCutoff('stars', stored, bright, CALIBRATED).value).toBe(750);
    // The faint session cuts at the same share of its own reference, 291,
    // not at 750, which would put every frame it holds under the cutoff.
    expect(resolveCutoff('stars', stored, faint, CALIBRATED).value).toBe(247);
  });

  it('keeps nothing for a cutoff left at its default', () => {
    const hfrs = [2, 2.2, 2.4, 2.8];
    expect(storedCutoff('hfr', 2.68, hfrs, CALIBRATED)).toBeNull();
    expect(storedCutoff('score', 50, [], CALIBRATED)).toBeNull();
    expect(storedCutoff('score', 62, [], CALIBRATED)).toBe(62);
  });

  it('spans a very long all-sessions view', () => {
    const many = Array.from({ length: 200_000 }, (_, i) => 1 + (i % 500));
    expect(cutoffScale('stars', many)).toEqual({ min: 1, max: 500, step: 1 });
  });
});
