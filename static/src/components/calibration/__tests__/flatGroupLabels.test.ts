import { describe, expect, it } from 'vitest';
import { timeLabel, windowLabel } from '../flatGroupLabels';

const utc = (text: string) => Date.parse(`${text}Z`) / 1000;
const PACIFIC = 'America/Los_Angeles';

describe('flat group times', () => {
  it('reads on the clock the flats were shot by, not in UTC', () => {
    // The C925's OIII dawn run, 11:58 to 12:00 UTC.
    expect(windowLabel(utc('2026-08-19T11:58:36'), utc('2026-08-19T12:00:56'), PACIFIC))
      .toBe('2026-08-19 04:58 – 2026-08-19 05:00 PDT');
    expect(timeLabel(utc('2026-12-11T14:30:00'), PACIFIC)).toBe('2026-12-11 06:30 PST');
    expect(timeLabel(utc('2026-12-11T14:30:00'), 'UTC')).toBe('2026-12-11 14:30 UTC');
  });

  it('names both zones when the clocks changed inside the window', () => {
    expect(windowLabel(utc('2026-10-25T12:00:00'), utc('2026-11-05T13:00:00'), PACIFIC))
      .toBe('2026-10-25 05:00 PDT – 2026-11-05 05:00 PST');
  });
});
