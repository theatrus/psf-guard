import { describe, expect, it } from 'vitest';
import { describeExportProgress } from '../../hooks/useExportJob';
import type { ExportSummary } from '../../api/types';
import { uncalibratedNote } from '../uncalibratedNote';

const summary: ExportSummary = {
  planned: 4,
  copied: 4,
  linked: 0,
  skipped_existing: 0,
  missing: 0,
  errors: 0,
  bytes: 0,
};

describe('uncalibratedNote', () => {
  it('says nothing when no light was left out', () => {
    expect(uncalibratedNote(summary)).toBeNull();
    expect(uncalibratedNote({ ...summary, uncalibrated: 0 })).toBeNull();
  });

  it('counts the lights left out and gives one reason', () => {
    expect(
      uncalibratedNote({ ...summary, uncalibrated: 1, uncalibrated_example: 'a.fits: No matching flat' })
    ).toBe("1 light that can't be calibrated left out (a.fits: No matching flat)");
    expect(uncalibratedNote({ ...summary, uncalibrated: 3 })).toBe(
      "3 lights that can't be calibrated left out"
    );
  });

  it('is part of a finished server export', () => {
    const message = describeExportProgress({
      running: false,
      stage: 'complete',
      destination: '/exports/m42',
      scope: 'project M42',
      total_files: 4,
      placed_files: 4,
      outcome: { ...summary, uncalibrated: 2, uncalibrated_example: 'b.fits: No matching flat' },
      started_at: 1,
      finished_at: 2,
      error: null,
    });
    expect(message).toBe(
      "Exported project M42: 4 file(s) placed, 2 lights that can't be calibrated left out " +
        '(b.fits: No matching flat) → /exports/m42'
    );
  });
});
