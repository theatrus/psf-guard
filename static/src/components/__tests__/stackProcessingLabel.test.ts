import { describe, expect, it } from 'vitest';
import type { StackActivityEntry } from '../../api/types';
import { applyingLabel, carriedLabel } from '../stackProcessingLabel';

describe('carriedLabel', () => {
  const run = (state: 'queued' | 'running'): StackActivityEntry => ({
    kind: 'processing', job_id: 'r'.repeat(64), database_id: 'db', project_id: 1, target_id: 2,
    state, label: 'M 44 · Ha', automatic: true, created_unix_seconds: 1,
    detail: state === 'queued' ? 'Waiting for the stack build of M 44 · L to finish' : 'BlurXTerminator',
    processed_units: state === 'running' ? 420 : 0, total_units: state === 'running' ? 1000 : 0,
    progress_label: state === 'running' ? 'BlurXTerminator · 42%' : null,
  });

  it('marks the previous stack result while the new one is processed', () => {
    expect(carriedLabel({ state: 'waiting', from_revision: 'r1' }))
      .toBe('From the previous stack · reprocessing · waiting for the stack to settle');
    expect(carriedLabel({ state: 'queued', from_revision: 'r1', run_id: 'r'.repeat(64) }, run('queued')))
      .toBe('From the previous stack · reprocessing · Waiting for the stack build of M 44 · L to finish');
    expect(carriedLabel({ state: 'running', from_revision: 'r1', run_id: 'r'.repeat(64) }, run('running')))
      .toBe('From the previous stack · reprocessing · BlurXTerminator · 42%');
    // The queue has not caught up yet: the server's own word.
    expect(carriedLabel({ state: 'running', from_revision: 'r1' }))
      .toBe('From the previous stack · reprocessing · running');
  });

  it('says why reprocessing failed, with the previous result still shown', () => {
    expect(carriedLabel({ state: 'failed', from_revision: 'r1', error: 'rc-astro is not installed' }))
      .toBe('From the previous stack · reprocessing failed: rc-astro is not installed');
    expect(carriedLabel({ state: 'failed', from_revision: 'r1' }))
      .toBe('From the previous stack · reprocessing failed');
  });
});

describe('applyingLabel', () => {
  it('says what a queued run waits for, and how many are ahead of it', () => {
    expect(applyingLabel({
      pending: true, queued: true, queue_position: 0,
      stage: 'Waiting for BlurXTerminator on M44 · G to finish',
      waiting_for: 'BlurXTerminator on M44 · G',
    })).toBe('Waiting for BlurXTerminator on M44 · G to finish');
    expect(applyingLabel({
      pending: true, queued: true, queue_position: 2,
      waiting_for: 'the stack build of M44 · Ha',
    })).toBe('Waiting for the stack build of M44 · Ha to finish · 2 ahead');
    // Nothing named holds the worker: the server's words stand.
    expect(applyingLabel({
      pending: true, queued: true, queue_position: 0, stage: 'Waiting for the stack worker',
    })).toBe('Waiting for the stack worker');
  });

  it('gives a running step its share done, or its name alone', () => {
    expect(applyingLabel({ pending: true, stage: 'RC-Astro StarXTerminator', fraction: 0.5 }))
      .toBe('Applying… 50% · StarXTerminator');
    expect(applyingLabel({ pending: true, fraction: 0.25 })).toBe('Applying… 25%');
    expect(applyingLabel({ pending: true, stage: 'Deconvolution' })).toBe('Applying… · Deconvolution');
    expect(applyingLabel({ pending: true })).toBe('Applying…');
    expect(applyingLabel(null)).toBe('Applying…');
  });
});
