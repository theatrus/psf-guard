import { describe, expect, it } from 'vitest';
import type { CalibrationLightMark, ImageQualityResult } from '../../api/types';
import {
  followRule,
  handChanges,
  parseScoreCutoff,
  parseSelectionRule,
  ruleSelection,
} from '../sequenceSelectionRules';

const frame = (image_id: number, quality_score: number, extra: Partial<ImageQualityResult> = {}) =>
  ({ image_id, quality_score, ...extra }) as ImageQualityResult;

describe('selection rules', () => {
  it('reads only known rules and cutoffs between 0 and 1 from the URL', () => {
    expect(parseSelectionRule('below-cutoff')).toBe('below-cutoff');
    expect(parseSelectionRule('threshold')).toBeNull();
    expect(parseSelectionRule(null)).toBeNull();
    expect(parseScoreCutoff(null)).toBe(0.5);
    expect(parseScoreCutoff('0.65')).toBe(0.65);
    expect(parseScoreCutoff('3')).toBe(1);
    expect(parseScoreCutoff('soon')).toBe(0.5);
  });

  it('answers for the frames given', () => {
    const images = [
      frame(1, 0.9),
      frame(2, 0.4, { category: 'likely_clouds' }),
      frame(3, 0.6, { flags: ['pointing_drift'] }),
      frame(4, 0.3, { regrade_reason: 'Clouds' }),
    ];
    const calibration = new Map<number, CalibrationLightMark>([[3, { image_id: 3, level: 'poor', codes: [] }]]);
    const inputs = { cutoff: 0.5, calibration };
    expect([...ruleSelection('below-cutoff', images, inputs)]).toEqual([2, 4]);
    expect([...ruleSelection('clouded', images, inputs)]).toEqual([2]);
    expect([...ruleSelection('off-target', images, inputs)]).toEqual([3]);
    expect([...ruleSelection('recommended', images, inputs)]).toEqual([4]);
    expect([...ruleSelection('poor-calibration', images, inputs)]).toEqual([3]);
  });

  it('follows a new answer and keeps what was changed by hand', () => {
    const before = new Set([3, 5, 7, 10]);
    // 5 removed by hand, 1 added by hand.
    const selection = new Set([1, 3, 7, 10]);
    const after = new Set([1, 3, 5, 7, 9, 10]);
    expect([...followRule(selection, before, after)].sort((a, b) => a - b)).toEqual([1, 3, 7, 9, 10]);
    // A frame the rule stops matching goes, unless it was added by hand.
    expect([...followRule(selection, before, new Set([10]))].sort((a, b) => a - b)).toEqual([1, 10]);
    expect(handChanges(selection, before)).toBe(2);
    expect(handChanges(before, before)).toBe(0);
  });
});
