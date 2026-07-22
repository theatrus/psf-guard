import { describe, expect, it } from 'vitest';
import type { Image } from '../../api/types';
import {
  groupImagesBySession,
  NO_EXPANDED_GROUPS,
  resolveExpandedGroups,
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
