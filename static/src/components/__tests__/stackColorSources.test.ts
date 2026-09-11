import { describe, expect, it } from 'vitest';
import type { StackColorJob, StackColorSource } from '../../api/types';
import { colorSourceKey, colorSourcesLabel, completedColorArtifact, resolveColorSources, sameColorSourceFamily } from '../stackColorSources';

const source = (key: string, revision = 'revision'): StackColorSource => ({
  role: 'red', filter_name: 'R', job_id: key, group_index: 0, artifact_revision: revision,
  label: `R (${key})`, exposure_group: { key, label: key, min_seconds: 30, max_seconds: 30 },
  accepted_frames: 10, reference_image_id: 1, sky_orientation: null, registration_transform: null,
});

describe('color source selection', () => {
  it('auto-selects only a unique candidate', () => {
    expect(resolveColorSources([source('short')], ['red'], {}).complete).toBe(true);
    expect(resolveColorSources([source('short'), source('long')], ['red'], {}).complete).toBe(false);
  });

  it('sends the exact selected immutable source reference', () => {
    const long = source('long');
    const result = resolveColorSources([source('short'), long], ['red'], { red: colorSourceKey(long) });
    expect(result.complete).toBe(true);
    expect(result.inputSources).toEqual({ red: { job_id: 'long', group_index: 0, artifact_revision: 'revision' } });
  });

  it('does not silently replace an explicit source whose artifact revision changed', () => {
    const original = source('short');
    const result = resolveColorSources([source('short', 'new')], ['red'], { red: colorSourceKey(original) });
    expect(result.complete).toBe(false);
    expect(result.inputSources).toEqual({});
  });

  it('matches saved families by role, raw filter, and group, not current revision or label', () => {
    expect(sameColorSourceFamily(source('short'), source('short', 'new'))).toBe(true);
    expect(sameColorSourceFamily(source('short'), source('long'))).toBe(false);
    expect(sameColorSourceFamily(source('short'), { ...source('short'), filter_name: 'Ha' })).toBe(false);
    expect(sameColorSourceFamily(source('short'), { ...source('short'), role: 'green' })).toBe(false);
  });

  it('shows raw filters for legacy saved sources with an empty label', () => {
    expect(colorSourcesLabel([{ ...source('short'), label: '', exposure_group: null }])).toBe('R');
    expect(colorSourcesLabel([{ ...source('short'), label: '' }])).toBe('R · short');
  });

  it('uses refreshed catalog staleness after a watched color job completes', () => {
    const watched = { job_id: 'color', state: 'completed', outdated: false } as StackColorJob;
    const refreshed = { ...watched, outdated: true, outdated_reason: 'Exposure grouping changed' };
    expect(completedColorArtifact(watched, [refreshed], undefined)).toBe(refreshed);
    expect(completedColorArtifact(watched, [], undefined)).toBe(watched);
  });
});
