import { describe, expect, it } from 'vitest';
import type { Image } from '../../api/types';
import { groupGridImages, imageGroupKey, resolveExpandedGroups } from '../imageGrouping';
import { nightOf } from '../observingNight';

function image(id: number, acquiredDate: number): Image {
  return {
    id,
    project_id: 1,
    project_name: 'Project',
    project_display_name: 'Project',
    target_id: 1,
    target_name: 'Target',
    acquired_date: acquiredDate,
    filter_name: 'HA',
    grading_status: 0,
    reject_reason: null,
    metadata: {},
    filesystem_path: null,
  };
}

/** 2026-10-`day` `hour`:`minute` local time at `offset` hours from UTC. */
const local = (offset: number, day: number, hour: number, minute = 0) =>
  Date.UTC(2026, 9, day, hour, minute) / 1000 - offset * 3600;
// Where the server splits a catalog that captures 19:30 to 05:00 local: the
// middle of its quiet hours (sky_coverage::night_boundary).
const boundaries: Record<number, number> = { [-7]: 19.5 * 3600, [-4]: 16.5 * 3600, 9: 3.5 * 3600 };
const labels = (groups: { filterName: string }[]) => groups.map((group) => group.filterName);
const ids = (groups: { images: Image[] }[]) => groups.map((group) => group.images.map((entry) => entry.id));

describe('date groups are observing nights', () => {
  for (const offset of [-7, -4, 9]) {
    it(`puts 23:30 and 01:30 local in one night at UTC${offset > 0 ? '+' : ''}${offset}`, () => {
      const scope = { nightBoundarySeconds: boundaries[offset] };
      const frames = [
        image(1, local(offset, 12, 23, 30)),
        image(2, local(offset, 13, 1, 30)),
        image(3, local(offset, 13, 23, 30)),
      ];
      const nights = groupGridImages(frames, 'date', 'oldest', scope);
      expect(labels(nights)).toEqual(['2026-10-13', '2026-10-12']);
      expect(ids(nights)).toEqual([[3], [1, 2]]);
      expect(labels(groupGridImages(frames, 'both', 'oldest', scope)))
        .toEqual(['HA - 2026-10-12', 'HA - 2026-10-13']);
      expect(labels(groupGridImages(frames, 'project+date', 'oldest', scope)))
        .toEqual(['Project - 2026-10-13', 'Project - 2026-10-12']);
      // The keys are the nights, so they hold across reloads.
      expect(nights.map(imageGroupKey)).toEqual(['2026-10-13', '2026-10-12']);
    });
  }

  it('starts the next night at the boundary itself', () => {
    const boundary = boundaries[-7];
    const split = Date.UTC(2026, 9, 13) / 1000 + boundary;
    expect(nightOf(split, boundary)).toBe('2026-10-13');
    expect(nightOf(split - 1, boundary)).toBe('2026-10-12');
    const nights = groupGridImages([image(1, split - 1), image(2, split)], 'date', 'oldest', {
      nightBoundarySeconds: boundary,
    });
    expect(labels(nights)).toEqual(['2026-10-13', '2026-10-12']);
    expect(ids(nights)).toEqual([[2], [1]]);
  });

  it('takes each database\'s own boundary in a merged list', () => {
    const west = { ...image(1, local(-7, 13, 1, 30)), db_id: 'west' };
    const east = { ...image(2, local(9, 13, 1, 30)), db_id: 'east' };
    const scope = { nightBoundarySeconds: new Map([['west', boundaries[-7]], ['east', boundaries[9]]]) };
    const nights = groupGridImages([west, east], 'date', 'oldest', scope);
    // Both frames belong to the evening of the 12th, each by its own
    // catalog's split.
    expect(new Set(labels(nights))).toEqual(new Set(['2026-10-12']));
    expect(ids(nights).flat().sort()).toEqual([1, 2]);
  });

  it('opens a night from an older link that named its UTC date', () => {
    // At UTC-4 the evening of the 12th runs past 00:00 UTC, so the old UTC
    // date groups cut it in two.
    const scope = { nightBoundarySeconds: boundaries[-4] };
    const frames = [
      image(1, local(-4, 12, 19, 30)),
      image(2, local(-4, 13, 1, 30)),
      image(3, local(-4, 13, 19, 30)),
    ];
    const nights = groupGridImages(frames, 'date', 'oldest', scope);
    expect(ids(nights)).toEqual([[3], [1, 2]]);
    // "2026-10-12" (UTC) held frame 1 only: its night opens.
    expect([...resolveExpandedGroups(nights, 'date', new Set(['2026-10-12']))]).toEqual(['2026-10-12']);
    // "2026-10-13" (UTC) held frames 2 and 3: both their nights open.
    expect([...resolveExpandedGroups(nights, 'date', new Set(['2026-10-13']))])
      .toEqual(['2026-10-13', '2026-10-12']);
    const both = groupGridImages(frames, 'both', 'oldest', scope);
    expect([...resolveExpandedGroups(both, 'both', new Set(['HA - 2026-10-12']))]).toEqual(['HA - 2026-10-12']);
  });
});
