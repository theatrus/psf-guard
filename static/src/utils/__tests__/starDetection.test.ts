import { describe, expect, it } from 'vitest';
import type { ProfileRecordView } from '../../api/types';
import { recordSpans, recordsThatStillApply } from '../starDetection';

function record(id: string, effectiveFrom: string | undefined, source: ProfileRecordView['source'] = 'import', received = '2026-11-02T12:00:00Z'): ProfileRecordView {
  return {
    id,
    effective_from: effectiveFrom,
    source,
    received_at: received,
    hocus_focus_version: 'v4',
    settings: {} as ProfileRecordView['settings'],
    hocus_focus_mode: 'simple',
    hocus_focus: {} as ProfileRecordView['hocus_focus'],
    notes: [],
  };
}

const day = (iso: string) => new Date(iso).toLocaleDateString(undefined, { dateStyle: 'medium' });

describe('recordSpans', () => {
  it('gives each record the frames it applies to, the earliest covering older frames', () => {
    const spans = recordSpans([
      record('sep9', '2026-09-09T12:00:00Z'),
      record('sep1', '2026-09-01T12:00:00Z', 'plugin:sync'),
    ]);
    expect(spans.get('sep1')).toBe(`Frames before ${day('2026-09-09T12:00:00Z')}`);
    expect(spans.get('sep9')).toBe(`Frames from ${day('2026-09-09T12:00:00Z')}`);
  });

  it('limits an all-frames record to frames before the first dated one', () => {
    const spans = recordSpans([
      record('all', undefined),
      record('sep1', '2026-09-01T12:00:00Z'),
      record('sep9', '2026-09-09T12:00:00Z'),
    ]);
    expect(spans.get('all')).toBe(`Frames before ${day('2026-09-01T12:00:00Z')}`);
    expect(spans.get('sep1')).toBe(
      `Frames from ${day('2026-09-01T12:00:00Z')} until ${day('2026-09-09T12:00:00Z')}`,
    );
    expect(recordSpans([record('only', undefined)]).get('only')).toBe('All frames');
  });

  it('says a record received earlier for the same date is replaced', () => {
    const spans = recordSpans([
      record('old', '2026-09-01T12:00:00Z', 'import', '2026-10-01T00:00:00Z'),
      record('new', '2026-09-01T12:00:00Z', 'import', '2026-11-01T00:00:00Z'),
    ]);
    expect(spans.get('new')).toBe('All frames');
    expect(spans.get('old')).toBe('Replaced by a newer record');
  });
});

describe('recordsThatStillApply', () => {
  const records = [
    record('all', undefined),
    record('sep1', '2026-09-01T12:00:00Z', 'plugin:sync'),
    record('sep9', '2026-09-09T12:00:00Z', 'plugin:director'),
  ];

  it('lists every dated record after an all-frames import, by its source', () => {
    expect(recordsThatStillApply(records)).toEqual([
      `Sync plugin record still applies from ${day('2026-09-01T12:00:00Z')}`,
      `Director plugin record still applies from ${day('2026-09-09T12:00:00Z')}`,
    ]);
  });

  it('lists only the records that start after a dated import', () => {
    expect(recordsThatStillApply(records, '2026-09-05T00:00:00Z')).toHaveLength(1);
    expect(recordsThatStillApply(records, '2026-10-01T00:00:00Z')).toEqual([]);
  });
});
