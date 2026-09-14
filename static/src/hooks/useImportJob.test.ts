import { describe, expect, it } from 'vitest';
import type { CalibrationImportOutcome, ImportJobProgress, ImportOutcome } from '../api/types';
import { describeImportProgress, importFinishedMessage } from './useImportJob';

function calibration(overrides: Partial<CalibrationImportOutcome> = {}): CalibrationImportOutcome {
  return { imported: 0, updated: 0, skipped_existing: 0, bias: 0, dark: 0, dark_flat: 0, flat: 0, ...overrides };
}

function outcome(overrides: Partial<ImportOutcome> = {}): ImportOutcome {
  return {
    dry_run: false,
    imported: 0,
    attached: 0,
    projects_created: 0,
    skipped_existing: 0,
    skipped_processed: 0,
    skipped_out_of_scope: 0,
    attach_summaries: [],
    project_summaries: [],
    calibration: calibration(),
    ...overrides,
  } as ImportOutcome;
}

function complete(o: ImportOutcome): ImportJobProgress {
  return {
    running: false,
    stage: 'complete',
    scanned_files: 0,
    total_files: 0,
    error: null,
    outcome: o,
  } as ImportJobProgress;
}

describe('describeImportProgress', () => {
  it('leads with calibration frames when no lights were imported', () => {
    const progress = complete(outcome({ calibration: calibration({ imported: 12, updated: 0, skipped_existing: 0 }) }));
    expect(describeImportProgress(progress)).toBe('Imported 12 calibration frame(s).');
  });

  it('keeps the light summary when lights and calibration both arrive', () => {
    const progress = complete(
      outcome({ imported: 5, attached: 5, calibration: calibration({ imported: 0, updated: 3, skipped_existing: 0 }) })
    );
    expect(describeImportProgress(progress)).toBe(
      'Imported 5 light frame(s) — 5 to existing target(s), 3 calibration frame(s).'
    );
  });

  it('reports nothing new when nothing changed', () => {
    const progress = complete(
      outcome({ skipped_existing: 4, calibration: calibration({ imported: 0, updated: 0, skipped_existing: 2 }) })
    );
    expect(describeImportProgress(progress)).toBe(
      'Imported 0 light frame(s) — nothing new, 4 already present, 2 calibration frame(s) unchanged.'
    );
  });
});

describe('importFinishedMessage', () => {
  it('stays quiet while the job runs', () => {
    const progress = { ...complete(outcome()), running: true, stage: 'importing' } as ImportJobProgress;
    expect(importFinishedMessage(progress, 'redcat')).toBeNull();
  });

  it('reports a finished calibration import', () => {
    const progress = complete(outcome({ calibration: calibration({ imported: 12, updated: 0, skipped_existing: 0 }) }));
    expect(importFinishedMessage(progress, 'redcat')).toBe('Imported 12 calibration frame(s).');
  });

  it('hands a finished preview to the confirm step', () => {
    const progress = complete(outcome({ dry_run: true }));
    expect(importFinishedMessage(progress, 'redcat')).toBe(
      'Preview ready for redcat — nothing is written until you confirm.'
    );
  });

  it('reads a failure as one', () => {
    const progress = { ...complete(outcome()), stage: 'error', error: 'disk full', outcome: null } as ImportJobProgress;
    expect(importFinishedMessage(progress, 'redcat')).toBe('Import failed: disk full');
  });
});
