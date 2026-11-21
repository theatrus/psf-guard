import { describe, expect, it } from 'vitest';
import { KNOWN_BANDPASSES, nextObjectiveBand } from '../planModel';

describe('the band a new objective starts on', () => {
  it('skips Luminance when no rig or library template covers it', () => {
    expect(nextObjectiveBand(KNOWN_BANDPASSES, new Set(), new Set(['blue']))).toBe('blue');
  });
  it('takes the next covered band the plan has no objective for', () => {
    expect(nextObjectiveBand(KNOWN_BANDPASSES, new Set(['red']), new Set(['red', 'h_alpha']))).toBe('h_alpha');
  });
  it('falls back to the first unused band when nothing is covered', () => {
    expect(nextObjectiveBand(KNOWN_BANDPASSES, new Set(['luminance']), new Set())).toBe(KNOWN_BANDPASSES[1].id);
  });
});
