import { describe, expect, it } from 'vitest';
import type { ImageQualityResult } from '../../api/types';
import { sequenceIssues } from '../sequenceIssues';

const frame = (image_id: number, extra: Partial<ImageQualityResult> = {}) =>
  ({ image_id, quality_score: 0.5, category: null, ...extra }) as ImageQualityResult;

describe('sequenceIssues', () => {
  it('names the frames behind each suspect pill, in the order given', () => {
    const issues = sequenceIssues([
      frame(1),
      frame(2, { category: 'likely_clouds', flags: ['likely_clouds', 'plate_solve_failed'] }),
      frame(3, { flags: ['pointing_jump'] }),
      frame(4, { flags: ['focus_drift', 'tracking_error'] }),
      frame(5, { flags: ['off_target'] }),
      // An older server sends the category alone.
      frame(6, { category: 'likely_clouds' }),
    ]);
    expect(issues.map(issue => [issue.label, issue.frames.map(f => f.image_id)])).toEqual([
      ['2 cloud events', [2, 6]],
      ['Focus drift', [4]],
      ['Tracking issues', [3, 4]],
      ['1 off target', [5]],
      ['1 unsolved', [2]],
    ]);
  });

  it('leaves out a pill with no frames', () => {
    expect(sequenceIssues([frame(1), frame(2, { flags: ['wind_shake'] })])).toEqual([]);
    expect(sequenceIssues([frame(1, { flags: ['likely_clouds'] })])[0].label).toBe('1 cloud event');
  });
});
