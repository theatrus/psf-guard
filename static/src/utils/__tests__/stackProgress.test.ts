import { describe, expect, it } from 'vitest';
import { stackGroupPercent } from '../stackProgress';

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
});
