import { describe, expect, it } from 'vitest';
import type { Image } from '../../api/types';
import {
  groupImagesBySession,
  NO_EXPANDED_GROUPS,
  resolveExpandedGroups,
  splitImageGroupsByExposure,
  imageGroupKey,
} from '../imageGrouping';

function image(id: number, acquiredDate: number, filter = 'HA'): Image {
  return {
    id,
    project_id: 1,
    project_name: 'Project',
    project_display_name: 'Project',
    target_id: 1,
    target_name: 'Target',
    acquired_date: acquiredDate,
    filter_name: filter,
    grading_status: 0,
    reject_reason: null,
    metadata: {},
    filesystem_path: null,
  };
}

describe('session image grouping', () => {
  it('splits at one-hour gaps and sorts the newest session first', () => {
    const start = Math.floor(new Date('2026-01-15T23:30:00').getTime() / 1000);
    const groups = groupImagesBySession([
      image(1, start),
      image(2, start + 120),
      image(3, start + 3 * 60 * 60),
    ]);

    expect(groups.map((group) => group.images.map((entry) => entry.id))).toEqual([[3], [1, 2]]);
  });

  it('opens only the newest session by default and preserves collapse-all', () => {
    const groups = [
      { filterName: 'newest', images: [image(2, 2)] },
      { filterName: 'older', images: [image(1, 1)] },
    ];

    expect([...resolveExpandedGroups(groups, 'session', new Set())]).toEqual(['newest']);
    expect([...resolveExpandedGroups(groups, 'session', new Set([NO_EXPANDED_GROUPS]))]).toEqual([]);
  });

  it('keeps a live session key stable when its end time changes', () => {
    const start = Math.floor(new Date('2026-01-15T20:00:00').getTime() / 1000);
    const before = groupImagesBySession([image(1, start), image(2, start + 60)])[0];
    const after = groupImagesBySession([
      image(1, start),
      image(2, start + 60),
      image(3, start + 120),
    ])[0];

    expect(after.key).toBe(before.key);
    expect(after.filterName).not.toBe(before.filterName);
  });
});

describe('server exposure partitions', () => {
  const short = { key: 'short', label: '30 s', min_seconds: 30, max_seconds: 30 };
  const long = { key: 'long', label: '300 s', min_seconds: 300, max_seconds: 300 };

  it('preserves disabled groups and only uses server group identities', () => {
    const unsplit = { filterName: 'HA', images: [image(1, 1), image(2, 2)] };
    expect(splitImageGroupsByExposure([unsplit])[0]).toBe(unsplit);
    const grouped = splitImageGroupsByExposure([{ ...unsplit, images: [
      { ...image(1, 1), exposure_group: short },
      { ...image(2, 2), exposure_group: long },
      { ...image(3, 3), exposure_group: short, metadata: { ExposureTime: 9999 } },
    ] }]);
    expect(grouped.map((group) => group.images.map((entry) => entry.id))).toEqual([[1, 3], [2]]);
    expect(grouped.map((group) => group.filterName)).toEqual(['HA · 30 s', 'HA · 300 s']);
    expect(imageGroupKey(grouped[0])).not.toBe(imageGroupKey(grouped[1]));
  });

  it('keeps identities across filters of the visible population and changed labels', () => {
    const before = splitImageGroupsByExposure([{ filterName: 'HA', images: [
      { ...image(1, 1), exposure_group: short }, { ...image(2, 2), exposure_group: long },
    ] }]);
    const after = splitImageGroupsByExposure([{ filterName: 'HA', images: [
      { ...image(1, 1), exposure_group: { ...short, label: '25–30 s', min_seconds: 25 } },
    ] }]);
    expect(imageGroupKey(after[0])).toBe(imageGroupKey(before[0]));
  });

  it('isolates enabled projects with the same exposure key and keeps disabled projects unsplit', () => {
    const groups = splitImageGroupsByExposure([{ filterName: 'All', images: [
      { ...image(1, 1), exposure_group: short },
      { ...image(2, 2), project_id: 2, exposure_group: short },
      { ...image(3, 3), project_id: 3 },
      { ...image(4, 4), project_id: 3 },
    ] }]);
    expect(groups.map((group) => group.images.map((entry) => entry.id))).toEqual([[1], [2], [3, 4]]);
  });

  it('preserves expanded groups through toggling and opens all bands of the newest session', () => {
    const base = [{ key: 'session', filterName: 'Session', images: [
      { ...image(1, 1), exposure_group: short }, { ...image(2, 2), exposure_group: long },
    ] }];
    const split = splitImageGroupsByExposure(base);
    expect(resolveExpandedGroups(split, 'filter', new Set(['session'])).size).toBe(2);
    expect(resolveExpandedGroups(split, 'session', new Set()).size).toBe(2);
    expect([...resolveExpandedGroups(base, 'filter', new Set(split.map(imageGroupKey)))]).toEqual(['session']);
    expect(resolveExpandedGroups(split, 'session', new Set([NO_EXPANDED_GROUPS])).size).toBe(0);
  });

  it('separates equal anchors for different targets and identifies their different ranges', () => {
    const groups = splitImageGroupsByExposure([{ filterName: 'R', images: [
      { ...image(1, 1, 'R'), target_name: 'M31', exposure_group: { ...short, key: '10', label: '10–11 s' } },
      { ...image(2, 2, 'R'), target_id: 2, target_name: 'M42', exposure_group: { ...short, key: '10', label: '10–18 s' } },
    ] }]);
    expect(groups.map((group) => group.filterName)).toEqual(['R · M31 · 10–11 s', 'R · M42 · 10–18 s']);
    expect(new Set(groups.map(imageGroupKey)).size).toBe(2);
  });

  it('splits date groups by their exact filter and includes the filter in the heading', () => {
    const groups = splitImageGroupsByExposure([{ filterName: '2026-09-11', images: [
      { ...image(1, 1, 'R'), exposure_group: short },
      { ...image(2, 2, 'G'), exposure_group: short },
    ] }]);
    expect(groups.map((group) => group.filterName)).toEqual(['2026-09-11 · R · 30 s', '2026-09-11 · G · 30 s']);
    expect(new Set(groups.map(imageGroupKey)).size).toBe(2);
  });
});
