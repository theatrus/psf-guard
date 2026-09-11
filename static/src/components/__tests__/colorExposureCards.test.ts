import { describe, expect, it } from 'vitest';
import type { StackColorKind, StackColorRole, StackColorSource, StackNarrowbandPalette } from '../../api/types';
import { buildColorExposureSets } from '../stackColorSources';
import { withRetainedColorExposureSets } from '../colorExposureCards';

const rgb: StackColorRole[] = ['red', 'green', 'blue'];

function source(role: StackColorRole, seconds: number, key = `${role}-${seconds}`): StackColorSource {
  return {
    role, filter_name: role, label: `${role} (${seconds} s)`,
    exposure_group: { key, label: `${seconds} s`, min_seconds: seconds, max_seconds: seconds },
    job_id: `${role}-${key}`, group_index: 0, artifact_revision: 'current', accepted_frames: 2,
    reference_image_id: null, sky_orientation: null, registration_transform: null,
  };
}

function recorded(
  sources: StackColorSource[], jobId = 'saved', created = 1,
  kind: StackColorKind = 'rgb', palette: StackNarrowbandPalette | null = null,
) {
  return { job_id: jobId, created_unix_seconds: created, sources, kind, palette };
}

describe('retained automatic color exposure cards', () => {
  it('fills missing family descriptors without duplicating rebuilt current roles', () => {
    const saved = rgb.map((role) => source(role, 30));
    const current = [{ ...saved[0], job_id: 'rebuilt', artifact_revision: 'new' }, saved[1]];
    const result = withRetainedColorExposureSets(buildColorExposureSets(current, rgb), [recorded(saved)], rgb);
    expect(result).toHaveLength(1);
    expect(result[0].retained).toBe(false);
    expect(result[0].candidates).toHaveLength(3);
    expect(result[0].candidates.find((entry) => entry.role === 'red')?.job_id).toBe('rebuilt');
    expect(result[0].key).toBe(buildColorExposureSets(saved, rgb)[0].key);
    expect(current).toHaveLength(2);
  });

  it('keeps a conflicting previous family separate instead of substituting the available channel', () => {
    const saved = rgb.map((role) => source(role, 30));
    const current = [source('red', 30, 'different-red'), source('green', 30), source('blue', 300)];
    const result = withRetainedColorExposureSets(buildColorExposureSets(current, rgb), [recorded(saved)], rgb);
    const retained = result.filter((set) => set.retained);
    expect(retained).toHaveLength(1);
    expect(retained[0].candidates).toEqual(buildColorExposureSets(saved, rgb)[0].candidates);
    expect(result.filter((set) => !set.retained)).toHaveLength(2);
  });

  it('uses current ranges for missing families that now live in another band', () => {
    const saved = rgb.map((role) => source(role, 30));
    const changedBlue = source('blue', 300, saved[2].exposure_group!.key);
    const current = buildColorExposureSets([saved[0], saved[1], changedBlue], rgb);
    const result = withRetainedColorExposureSets(current, [recorded(saved)], rgb);
    expect(result).toHaveLength(3);
    expect(result.filter((set) => !set.retained).map((set) => set.candidates.length)).toEqual([2, 1]);
    expect(result[2].retained).toBe(true);
  });

  it('keeps mixed-length and unknown custom combinations out of automatic sets', () => {
    const mixed = [source('red', 30), source('green', 300), source('blue', 300)];
    const unknown = rgb.map((role) => ({ ...source(role, 30), exposure_group: null }));
    expect(withRetainedColorExposureSets([], [recorded(mixed), recorded(unknown)], rgb)).toEqual([]);
    expect(withRetainedColorExposureSets([], [recorded(mixed.slice(0, 2))], rgb)).toEqual([]);
  });

  it('retains one card per family across saved and active revisions', () => {
    const saved = rgb.map((role) => source(role, 30));
    const rebuilt = saved.map((entry) => ({ ...entry, job_id: 'new', artifact_revision: 'new' }));
    const result = withRetainedColorExposureSets([], [recorded(saved), recorded(rebuilt, 'active', 2)], rgb);
    expect(result).toHaveLength(1);
    expect(result[0].retained).toBe(true);
    expect(result[0].candidates.every((entry) => entry.job_id === 'new')).toBe(true);
  });

  it('does not resolve duplicate-role ambiguity with an older saved selection', () => {
    const saved = rgb.map((role) => source(role, 30));
    const current = buildColorExposureSets([saved[0], source('red', 30, 'other-red'), saved[1]], rgb);
    const result = withRetainedColorExposureSets(current, [recorded(saved)], rgb);
    expect(result).toHaveLength(2);
    expect(result[0].candidates).toHaveLength(3);
    expect(result[1].retained).toBe(true);
  });

  it('requires each saved recipe to be complete and within the allowed role pool', () => {
    const lrgb: StackColorRole[] = ['luminance', ...rgb];
    const sho: StackColorRole[] = ['ha', 'oiii', 'sii'];
    const incompleteLrgb = recorded(rgb.map((role) => source(role, 30)), 'lrgb', 1, 'lrgb');
    const incompleteSho = recorded([source('ha', 30), source('oiii', 30)], 'sho', 1, 'narrowband', 'sho');
    const completeLrgb = recorded(lrgb.map((role) => source(role, 30)), 'lrgb', 1, 'lrgb');
    const completeSho = recorded(sho.map((role) => source(role, 30)), 'sho', 1, 'narrowband', 'sho');
    expect(withRetainedColorExposureSets([], [incompleteLrgb], lrgb)).toEqual([]);
    expect(withRetainedColorExposureSets([], [incompleteSho], sho)).toEqual([]);
    expect(withRetainedColorExposureSets([], [completeLrgb], lrgb)).toHaveLength(1);
    expect(withRetainedColorExposureSets([], [completeSho], sho)).toHaveLength(1);
    expect(withRetainedColorExposureSets([], [completeLrgb], rgb)).toEqual([]);
  });

  it('retains complete HOO jobs without requiring SII in the allowed narrowband pool', () => {
    const roles: StackColorRole[] = ['ha', 'oiii', 'sii'];
    const sources = [source('ha', 30), source('oiii', 30)];
    const jobs = [recorded(sources, 'hoo', 1, 'narrowband', 'hoo')];
    const retained = withRetainedColorExposureSets([], jobs, roles);
    expect(retained).toHaveLength(1);
    expect(retained[0].retained).toBe(true);
    expect(retained[0].candidates).toHaveLength(2);
    const augmented = withRetainedColorExposureSets(buildColorExposureSets([sources[0]], roles), jobs, roles);
    expect(augmented).toHaveLength(1);
    expect(augmented[0].retained).toBe(false);
    expect(augmented[0].candidates).toHaveLength(2);
  });

  it('preserves current SII while restoring a missing HOO family', () => {
    const roles: StackColorRole[] = ['ha', 'oiii', 'sii'];
    const saved = [source('ha', 30), source('oiii', 30)];
    const sii = source('sii', 40);
    const result = withRetainedColorExposureSets(
      buildColorExposureSets([saved[0], sii], roles),
      [recorded(saved, 'hoo', 1, 'narrowband', 'foraxx-hoo')], roles,
    );
    expect(result).toHaveLength(1);
    expect(result[0].retained).toBe(false);
    expect(result[0].candidates).toContain(sii);
    expect(result[0].candidates).toHaveLength(3);
    expect(result[0].maxSeconds).toBe(40);
  });

  it('deduplicates HOO jobs already represented by a current three-role narrowband set', () => {
    const roles: StackColorRole[] = ['ha', 'oiii', 'sii'];
    const saved = [source('ha', 30), source('oiii', 30)];
    const current = buildColorExposureSets([...saved, source('sii', 30)], roles);
    const result = withRetainedColorExposureSets(current, [recorded(saved, 'hoo', 1, 'narrowband', 'hoo')], roles);
    expect(result).toHaveLength(1);
    expect(result[0].key).toBe(current[0].key);
  });

  it('includes optional current SII in the full range test before restoring a HOO family', () => {
    const roles: StackColorRole[] = ['ha', 'oiii', 'sii'];
    const saved = [source('ha', 30), source('oiii', 50)];
    const current = buildColorExposureSets([saved[0], source('sii', 20)], roles);
    const result = withRetainedColorExposureSets(current, [recorded(saved, 'hoo', 1, 'narrowband', 'hoo')], roles);
    expect(result).toHaveLength(2);
    expect(result[0].candidates).toHaveLength(2);
    expect(result[1].retained).toBe(true);
  });
});
