import { describe, expect, it } from 'vitest';
import { masterBuildLabel, stackGroupPercent } from '../stackProgress';

const group = { state: 'running' as const, eligible_frames: 10, processed_frames: 4, final_pass: null };

describe('stack progress', () => {
  it('counts the final pass reads, so the bar keeps moving after the frames', () => {
    // Calibration settled, then 4 frames: 14 of 50 units (10 + 10 live + 30 final).
    expect(stackGroupPercent(group, undefined)).toBeCloseTo(28, 5);
    expect(stackGroupPercent(group, { final_pass: 'draft' } as never)).toBeCloseTo(70, 5);
    const final = { ...group, processed_frames: 10, final_pass: { pass: 2, passes: 3, frame: 3, frames: 8 } };
    // (10 + 10 + 11) of (10 + 10 + 24)
    expect(stackGroupPercent(final, undefined)).toBeCloseTo(70.45, 1);
    expect(stackGroupPercent({ ...final, state: 'ready' as const }, undefined)).toBe(100);
  });

  it('grows through the calibration share while masters build', () => {
    const dark = { kind: 'dark' as const, stage: 'integrate' as const, done: 4, total: 20, fraction: 0.6, build: 1 };
    const building = { ...group, processed_frames: 0, phase: 'calibration', calibration_progress: dark };
    // The first build at 0.6 fills 0.3 of the share: 3 of 50 units.
    expect(stackGroupPercent(building, undefined)).toBeCloseTo(6, 5);
    expect(masterBuildLabel(dark)).toBe('master dark · integrating frame 5/20');
    expect(masterBuildLabel({ kind: 'flat', filter: 'Ha', stage: 'combine', done: 2, total: 8, fraction: 0.8 }))
      .toBe('master flat Ha · combining tile 3/8');
    // A later build starts from where the earlier ones left the share.
    const next = { ...building, calibration_progress: { ...dark, build: 2, stage: 'read' as const, done: 0, fraction: 0 } };
    expect(stackGroupPercent(next, undefined)).toBeGreaterThan(stackGroupPercent(building, undefined));
    // Once stacking starts the share is settled.
    expect(stackGroupPercent({ ...building, phase: 'stacking' }, undefined)).toBeCloseTo(20, 5);
  });
});
