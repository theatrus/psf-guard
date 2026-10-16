import { describe, expect, it } from 'vitest';
import { timeLabel, windowLabel } from '../flatGroupLabels';

const utc = (text: string) => Date.parse(`${text}Z`) / 1000;
const PACIFIC = 'America/Los_Angeles';
/** The C925's clock as its frames record it: PDT, then PST from 11-01. */
const C925 = [
  { from: utc('2026-03-08T10:00:00'), offset_seconds: -7 * 3600 },
  { from: utc('2026-11-01T09:00:00'), offset_seconds: -8 * 3600 },
];

describe('flat group times', () => {
  it('reads on the rig clock its frames record', () => {
    // The C925's OIII dawn run, 11:58 to 12:00 UTC.
    expect(windowLabel(utc('2026-08-19T11:58:36'), utc('2026-08-19T12:00:56'), C925))
      .toBe('2026-08-19 04:58 – 2026-08-19 05:00 UTC−7');
    expect(timeLabel(utc('2026-12-11T14:30:00'), C925)).toBe('2026-12-11 06:30 UTC−8');
    // Before its first recorded run, the first run's clock.
    expect(timeLabel(utc('2026-01-05T14:30:00'), C925)).toBe('2026-01-05 07:30 UTC−7');
    expect(timeLabel(utc('2026-01-05T14:30:00'), [{ from: 0, offset_seconds: 5.5 * 3600 }]))
      .toBe('2026-01-05 20:00 UTC+5:30');
    expect(timeLabel(utc('2026-01-05T14:30:00'), [{ from: 0, offset_seconds: 0 }]))
      .toBe('2026-01-05 14:30 UTC');
  });

  it('names both offsets when the rig clock changed inside the window', () => {
    expect(windowLabel(utc('2026-10-25T12:00:00'), utc('2026-11-05T13:00:00'), C925))
      .toBe('2026-10-25 05:00 UTC−7 – 2026-11-05 05:00 UTC−8');
  });

  it('reads on the viewer clock when the rig records none', () => {
    expect(windowLabel(utc('2026-08-19T11:58:36'), utc('2026-08-19T12:00:56'), [], PACIFIC))
      .toBe('2026-08-19 04:58 – 2026-08-19 05:00 PDT');
    expect(timeLabel(utc('2026-12-11T14:30:00'), undefined, PACIFIC)).toBe('2026-12-11 06:30 PST');
    expect(windowLabel(utc('2026-10-25T12:00:00'), utc('2026-11-05T13:00:00'), undefined, PACIFIC))
      .toBe('2026-10-25 05:00 PDT – 2026-11-05 05:00 PST');
  });
});
