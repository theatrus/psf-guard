import { describe, expect, it } from 'vitest';
import { applyingLabel } from '../stackProcessingLabel';

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
