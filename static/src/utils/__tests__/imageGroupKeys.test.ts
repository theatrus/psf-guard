import { describe, expect, it } from 'vitest';
import type { Image, ProjectMosaic } from '../../api/types';
import {
  groupGridImages,
  groupImagesByPanel,
  groupImagesBySession,
  imageGroupKey,
  resolveExpandedGroups,
} from '../imageGrouping';

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

const short = { key: 'short', label: '30 s', min_seconds: 30, max_seconds: 30 };
const at = Date.UTC(2026, 9, 12, 22) / 1000;
// Two catalogs whose ids and names coincide: one rig family, two rigs.
const twin = (db: string, id: number) => ({ ...image(id, at + id), db_id: db, exposure_group: short });
const merged = [twin('askar', 1), twin('esprit', 2)];
const ids = (groups: { images: Image[] }[]) => groups.map((group) => group.images.map((entry) => entry.id));

describe('group keys across databases', () => {
  const modes = ['filter', 'session', 'date', 'both', 'project', 'project+filter', 'project+date', 'project+date+filter'] as const;
  for (const mode of modes) {
    it(`keeps two databases with one target id and project name apart in ${mode} groups`, () => {
      const groups = groupGridImages(merged, mode);
      expect(ids(groups).flat().sort(), mode).toEqual([1, 2]);
      expect(ids(groups).every((group) => group.length === 1), mode).toBe(true);
      expect(new Set(groups.map(imageGroupKey)).size, mode).toBe(2);
      for (const group of groups) {
        const db = (group.images[0] as { db_id?: string }).db_id!;
        expect(imageGroupKey(group), mode).toContain(db);
      }
    });
  }

  it('keeps two databases apart in session streams', () => {
    // Same target id, project name and filter, frames a second apart.
    expect(ids(groupImagesBySession(merged))).toEqual([[2], [1]]);
  });

  it('keeps two databases apart under a mosaic\'s panels', () => {
    const mosaic = { panels: [{ panel_id: 'r1c1', target_id: 1, target_name: 'Panel 1' }] } as unknown as ProjectMosaic;
    const groups = groupImagesByPanel(merged, mosaic, (images) => groupGridImages(images, 'filter'));
    expect(ids(groups)).toEqual([[1], [2]]);
    expect(new Set(groups.map(imageGroupKey)).size).toBe(2);
  });
});

describe('group keys in one database', () => {
  it('names projects by id, so two projects of one name stay apart', () => {
    const groups = groupGridImages([
      { ...image(1, at), project_display_name: 'M31' },
      { ...image(2, at), project_id: 2, project_display_name: 'M31' },
    ], 'project');
    expect(groups.map((group) => group.filterName)).toEqual(['M31', 'M31']);
    expect(ids(groups)).toEqual([[1], [2]]);
  });

  it('keeps filter, date and session keys as they were', () => {
    const one = [image(1, at), image(2, at + 60)];
    expect(groupGridImages(one, 'filter').map(imageGroupKey)).toEqual(['HA']);
    expect(groupGridImages(one, 'date').map(imageGroupKey)).toEqual(['2026-10-12']);
    expect(groupGridImages(one, 'both').map(imageGroupKey)).toEqual(['HA - 2026-10-12']);
    expect(groupGridImages(one, 'session').map(imageGroupKey)).toEqual([`Project|1|HA|${at}`]);
  });

  it('opens project groups from links that named them by display name', () => {
    const one = [
      { ...image(1, at), project_display_name: 'Alpha', exposure_group: short },
      { ...image(2, at), project_id: 2, project_display_name: 'Beta', exposure_group: short },
    ];
    const oldNames = {
      project: 'Beta',
      'project+filter': 'Beta - HA',
      'project+date': 'Beta - 2026-10-12',
      'project+date+filter': 'Beta - 2026-10-12 - HA',
    } as const;
    for (const [mode, oldName] of Object.entries(oldNames) as [keyof typeof oldNames, string][]) {
      const groups = groupGridImages(one, mode);
      const beta = imageGroupKey(groups.find((group) => group.images[0].id === 2)!);
      // The old unsplit group's name, and the old exposure sub-group's key.
      const oldExposure = JSON.stringify(['exposure', oldName, 2, 1, 'HA', 'short']);
      expect([...resolveExpandedGroups(groups, mode, new Set([oldName]))], mode).toEqual([beta]);
      expect([...resolveExpandedGroups(groups, mode, new Set([oldExposure]))], mode).toEqual([beta]);
      expect([...resolveExpandedGroups(groups, mode, new Set([beta]))], mode).toEqual([beta]);
    }
  });
});
