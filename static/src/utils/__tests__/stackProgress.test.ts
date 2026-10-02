import { describe, expect, it } from 'vitest';
import { masterBuildLabel, stackGroupPercent } from '../stackProgress';

const group = { state: 'running' as const, eligible_frames: 10, processed_frames: 4, final_pass: null };

describe('stack progress', () => {
  it('counts the final pass reads, so the bar keeps moving after the frames', () => {
    // 4 of 40 reads: 10 live, 30 final.
    expect(stackGroupPercent(group, undefined)).toBe(10);
    expect(stackGroupPercent(group, { final_pass: 'draft' } as never)).toBe(40);
    const final = { ...group, processed_frames: 10, final_pass: { pass: 2, passes: 3, frame: 3, frames: 8 } };
    // (10 + 11) of (10 + 24)
    expect(stackGroupPercent(final, undefined)).toBeCloseTo(61.76, 1);
    expect(stackGroupPercent({ ...final, state: 'ready' as const }, undefined)).toBe(100);
  });

  it('follows the master being built while calibration runs', () => {
    const dark = { kind: 'dark' as const, frame: 5, frames: 20, pass: 2, passes: 2 };
    const building = { ...group, processed_frames: 0, phase: 'calibration', calibration_progress: dark };
    // 24 of 40 reads finished; the 25th is under way.
    expect(stackGroupPercent(building, undefined)).toBe(60);
    expect(masterBuildLabel(dark)).toBe('master dark · pass 2/2 · frame 5/20');
    expect(masterBuildLabel({ kind: 'flat', filter: 'Ha', frame: 3, frames: 30, pass: 1, passes: 1 }))
      .toBe('master flat Ha · frame 3/30');
    // Once stacking starts the bar is the channel's again.
    expect(stackGroupPercent({ ...building, phase: 'stacking' }, undefined)).toBe(0);
  });
});
