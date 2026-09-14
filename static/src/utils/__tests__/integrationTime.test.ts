import { describe, expect, it } from 'vitest';
import { formatIntegration, totalIntegration } from '../integrationTime';

describe('formatIntegration', () => {
  it('reads as hours and minutes', () => {
    expect(formatIntegration(45)).toBe('45 s');
    expect(formatIntegration(59.6)).toBe('60 s');
    expect(formatIntegration(600)).toBe('10m');
    expect(formatIntegration(3600)).toBe('1h');
    expect(formatIntegration(3600 * 2 + 288)).toBe('2h 5m');
    expect(formatIntegration(3600 * 12 + 60 * 40)).toBe('12h 40m');
  });

  it('handles nothing gracefully', () => {
    expect(formatIntegration(undefined)).toBe('—');
    expect(formatIntegration(Number.NaN)).toBe('—');
    expect(totalIntegration([600, undefined, 1200])).toBe(1800);
  });
});
