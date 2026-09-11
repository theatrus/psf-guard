import { describe, expect, it } from 'vitest';
import type { StackColorJob, StackColorRole, StackColorSource } from '../../api/types';
import { buildColorExposureSets, colorSourceKey, colorSourcesLabel, completedColorArtifact, resolveColorSources, sameColorSourceFamily } from '../stackColorSources';

const source = (key: string, revision = 'revision'): StackColorSource => ({
  role: 'red', filter_name: 'R', job_id: key, group_index: 0, artifact_revision: revision,
  label: `R (${key})`, exposure_group: { key, label: key, min_seconds: 30, max_seconds: 30 },
  accepted_frames: 10, reference_image_id: 1, sky_orientation: null, registration_transform: null,
});

const rgb: StackColorRole[] = ['red', 'green', 'blue'];
const interval = (
  role: StackColorRole, min: number | null, max: number | null = min, key = `${role}-${min}`,
): StackColorSource => ({
  ...source(key), role, filter_name: role,
  exposure_group: { key, label: 'Display label', min_seconds: min, max_seconds: max },
});

describe('automatic color exposure sets', () => {
  it('matches actual seconds across unrelated filter-local family keys', () => {
    const candidates = [interval('red', 30, 30, 'exposure-12'), interval('green', 30, 30, 'unrelated'), interval('blue', 30, 30, 'third')];
    const sets = buildColorExposureSets(candidates, rgb);
    expect(sets).toHaveLength(1);
    expect(sets[0]).toMatchObject({ known: true, label: '30 s', minSeconds: 30, maxSeconds: 30 });
    expect(resolveColorSources(sets[0].candidates, rgb, {}).complete).toBe(true);
  });

  it('tolerates exposure jitter but separates short and long templates', () => {
    const candidates = [interval('red', 30), interval('green', 30.1), interval('blue', 29.9),
      interval('red', 300), interval('green', 300.1), interval('blue', 299.9)];
    const sets = buildColorExposureSets(candidates, rgb);
    expect(sets).toHaveLength(2);
    expect(sets.map((set) => set.candidates.length)).toEqual([3, 3]);
    expect(sets.map((set) => set.label)).toEqual(['29.9 s - 30.1 s', '299.9 s - 300.1 s']);
  });

  it('separates exactly twofold exposure differences', () => {
    const sets = buildColorExposureSets([interval('red', 30), interval('green', 60), interval('blue', 119.9)], rgb);
    expect(sets.map((set) => [set.minSeconds, set.maxSeconds])).toEqual([[30, 30], [60, 119.9]]);
  });

  it('retains three independently usable exposure tiers', () => {
    const sets = buildColorExposureSets([10, 100, 1000].flatMap((seconds) => rgb.map((role) => interval(role, seconds))), rgb);
    expect(sets.map((set) => set.label)).toEqual(['10 s', '100 s', '1000 s']);
    expect(sets.every((set) => resolveColorSources(set.candidates, rgb, {}).complete)).toBe(true);
  });

  it('does not chain adjacent exposure gaps into a wide group', () => {
    const sets = buildColorExposureSets([interval('red', 10), interval('green', 15), interval('blue', 25)], rgb);
    expect(sets.map((set) => [set.minSeconds, set.maxSeconds])).toEqual([[10, 15], [25, 25]]);
    expect(sets.every((set) => resolveColorSources(set.candidates, rgb, {}).complete)).toBe(false);
  });

  it('requires the entire candidate interval to fit the full band', () => {
    const sets = buildColorExposureSets([interval('red', 30, 50), interval('green', 31, 61), interval('blue', 32, 60)], rgb);
    expect(sets.map((set) => [set.minSeconds, set.maxSeconds])).toEqual([[30, 50], [31, 61]]);
    expect(sets[0].candidates.map((candidate) => candidate.role)).toEqual(['red']);
    expect(sets[1].candidates).toHaveLength(2);
  });

  it('keeps incomplete bands and duplicate roles without selecting an arbitrary candidate', () => {
    const candidates = [interval('red', 30, 30, 'one'), interval('red', 30, 30, 'two'), interval('green', 30), interval('blue', 300)];
    const sets = buildColorExposureSets(candidates, rgb);
    expect(sets.map((set) => set.candidates.length)).toEqual([3, 1]);
    const short = resolveColorSources(sets[0].candidates, rgb, {});
    expect(short.complete).toBe(false);
    expect(short.inputSources.red).toBeUndefined();
    expect(short.inputSources.green).toBeDefined();
  });

  it('separates unknown and invalid bounds from known bands', () => {
    const unknown = [
      interval('red', null), interval('green', 0), interval('blue', -1),
      interval('red', 30, null), interval('green', 30, Infinity), interval('blue', NaN),
      interval('red', 40, 30), { ...interval('green', 30), exposure_group: null },
    ];
    const sets = buildColorExposureSets([interval('red', 30), ...unknown], rgb);
    expect(sets).toHaveLength(2);
    expect(sets[0]).toMatchObject({ known: true, label: '30 s' });
    expect(sets[1]).toMatchObject({ known: false, label: 'Unknown exposure', minSeconds: null, maxSeconds: null });
    expect(sets[1].candidates).toHaveLength(unknown.length);
  });

  it('never marks an individually twofold or wider interval as safe to compose', () => {
    const sets = buildColorExposureSets([interval('red', 30, 60), interval('green', 10, 300), interval('blue', 30)], rgb);
    expect(sets).toHaveLength(2);
    expect(sets[1]).toMatchObject({ known: false, label: 'Mixed exposure', minSeconds: 10, maxSeconds: 300 });
    expect(sets[1].candidates).toHaveLength(2);
  });

  it('filters unused roles before constructing bands', () => {
    const sets = buildColorExposureSets([interval('ha', 1), ...rgb.map((role) => interval(role, 30))], rgb);
    expect(sets).toHaveLength(1);
    expect(sets[0].candidates).toHaveLength(3);
    expect(buildColorExposureSets([interval('ha', 30)], rgb)).toEqual([]);
    expect(buildColorExposureSets([interval('red', 30)], [])).toEqual([]);
  });

  it('keeps set identities across artifact rebuilds and display-duration changes', () => {
    const candidates = rgb.map((role) => interval(role, 30));
    const first = buildColorExposureSets(candidates, rgb)[0];
    const rebuilt = candidates.map((candidate) => ({
      ...candidate, job_id: `new-${candidate.job_id}`, group_index: 7, artifact_revision: 'new-revision',
      label: 'Changed display', exposure_group: { ...candidate.exposure_group!, label: 'Changed display', min_seconds: 30.1, max_seconds: 30.2 },
    }));
    const next = buildColorExposureSets(rebuilt, rgb)[0];
    expect(next.key).toBe(first.key);
    expect(next.label).not.toBe(first.label);
    expect(buildColorExposureSets([{ ...candidates[0], filter_name: 'Different red' }, ...candidates.slice(1)], rgb)[0].key).not.toBe(first.key);
    expect(buildColorExposureSets([{ ...candidates[0], exposure_group: { ...candidates[0].exposure_group!, key: 'new-family' } }, ...candidates.slice(1)], rgb)[0].key).not.toBe(first.key);
  });

  it('is deterministic under candidate and role input order without mutating inputs', () => {
    const candidates = [interval('red', 30), interval('blue', 300), interval('green', 30.1), interval('blue', 30)];
    const original = [...candidates];
    const expected = buildColorExposureSets(candidates, rgb);
    expect(buildColorExposureSets([...candidates].reverse(), [...rgb].reverse())).toEqual(expected);
    expect(candidates).toEqual(original);
  });
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
