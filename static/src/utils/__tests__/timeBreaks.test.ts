import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { hourStepFor, timeMarks } from '../timeBreaks';

// Nights split at 18:00 UTC; the test's clock runs in UTC (see vitest setup)
// unless a test says otherwise, so hours read the same as the data.
const BOUNDARY = 18 * 3600;
const at = (iso: string) => Date.parse(iso) / 1000;
const every = (startIso: string, minutes: number, count: number) =>
  Array.from({ length: count }, (_, i) => ({ acquired_date: at(startIso) + i * minutes * 60 }));

describe('time breaks', () => {
  beforeEach(() => {
    vi.stubEnv('TZ', 'UTC');
  });
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it('spaces hour marks by how often frames come', () => {
    expect(hourStepFor(every('2026-10-14T20:00:00Z', 5, 30), BOUNDARY)).toBe(1);
    expect(hourStepFor(every('2026-10-14T20:00:00Z', 10, 30), BOUNDARY)).toBe(1);
    expect(hourStepFor(every('2026-10-14T20:00:00Z', 20, 30), BOUNDARY)).toBe(2);
    expect(hourStepFor(every('2026-10-14T20:00:00Z', 30, 20), BOUNDARY)).toBe(3);
    expect(hourStepFor(every('2026-10-14T20:00:00Z', 60, 10), BOUNDARY)).toBeNull();
    expect(hourStepFor([{ acquired_date: at('2026-10-14T20:00:00Z') }], BOUNDARY)).toBeNull();
  });

  it('breaks at the night boundary, not at midnight, and at each hour', () => {
    // 22:30 to 01:30 on the night of the 14th, then the next evening.
    const times = [
      ...every('2026-10-14T22:30:00Z', 10, 19),
      ...every('2026-10-15T20:00:00Z', 10, 7),
    ];
    const marks = timeMarks(times, BOUNDARY);
    expect(marks.hourStep).toBe(1);
    const breaks = [...marks.breaks.entries()].map(([index, mark]) => [index, mark.kind, mark.label]);
    expect(breaks).toEqual([
      [3, 'hour', '23:00'],
      [9, 'hour', '00:00'],
      [15, 'hour', '01:00'],
      [19, 'night', expect.stringMatching(/Oct.*15|15.*Oct/)],
      [25, 'hour', '21:00'],
    ]);
    // The second night takes the alternate band.
    expect(marks.altNight.has(18)).toBe(false);
    expect(marks.altNight.has(19)).toBe(true);
    expect(marks.altNight.has(25)).toBe(true);
  });

  it('marks the same crossings newest first', () => {
    const times = every('2026-10-14T22:30:00Z', 10, 12).reverse();
    const marks = timeMarks(times, BOUNDARY);
    // 00:20 ... 23:00 ... 22:30: the run into 23:00 starts at the 23:50 frame.
    expect([...marks.breaks.entries()].map(([index, mark]) => [index, mark.label])).toEqual([
      [3, '23:00'],
      [9, '22:00'],
    ]);
  });

  it('takes the night the image list names over the catalog split', () => {
    // A rig west of Greenwich: its night began at 20:05 UTC local noon,
    // so 19:30 UTC still belongs to the night before, though the catalog's
    // 18:00 UTC split would put it in the next one.
    const frames = [
      { acquired_date: at('2026-08-19T18:30:00Z'), night: '2026-08-18' },
      { acquired_date: at('2026-08-19T19:30:00Z'), night: '2026-08-18' },
      { acquired_date: at('2026-08-19T20:30:00Z'), night: '2026-08-19' },
    ];
    const marks = timeMarks(frames, BOUNDARY);
    expect([...marks.breaks.entries()].map(([index, mark]) => [index, mark.kind])).toEqual([[2, 'night']]);
  });

  it('marks nights alone when asked for no hours', () => {
    const times = [
      ...every('2026-10-14T22:30:00Z', 10, 19),
      ...every('2026-10-15T20:00:00Z', 10, 7),
    ];
    const marks = timeMarks(times, BOUNDARY, { hours: false });
    expect(marks.hourStep).toBeNull();
    expect([...marks.breaks.values()].map((mark) => mark.kind)).toEqual(['night']);
  });

  it('skips frames without a capture time', () => {
    const marks = timeMarks([{ acquired_date: null }, { acquired_date: undefined }], BOUNDARY);
    expect(marks.breaks.size).toBe(0);
    expect(marks.hourStep).toBeNull();
  });
});
