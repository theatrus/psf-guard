import { describe, expect, it } from 'vitest';
import { formatRelativeTime } from '../relativeTime';

const NOW = Date.UTC(2026, 6, 24, 12, 0, 0);
const NOW_SECONDS = NOW / 1000;

describe('formatRelativeTime', () => {
  it('uses compact units from minutes through years', () => {
    expect(formatRelativeTime(NOW_SECONDS - 30, NOW)).toBe('just now');
    expect(formatRelativeTime(NOW_SECONDS - 5 * 60, NOW)).toBe('5 mins ago');
    expect(formatRelativeTime(NOW_SECONDS - 2 * 60 * 60, NOW)).toBe('2 hrs ago');
    expect(formatRelativeTime(NOW_SECONDS - 24 * 60 * 60, NOW)).toBe('1 day ago');
    expect(formatRelativeTime(NOW_SECONDS - 3 * 7 * 24 * 60 * 60, NOW)).toBe('3 wks ago');
    expect(formatRelativeTime(NOW_SECONDS - 2 * 365 * 24 * 60 * 60, NOW)).toBe('2 yrs ago');
  });

  it('handles future and missing timestamps', () => {
    expect(formatRelativeTime(NOW_SECONDS + 2 * 60 * 60, NOW)).toBe('in 2 hrs');
    expect(formatRelativeTime(null, NOW)).toBe('Time unknown');
  });
});
