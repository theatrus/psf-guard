import { describe, expect, it } from 'vitest';
import type { ProjectOverview, TargetOverview } from '../../api/types';
import {
  loadProjectSeenState,
  markerForProject,
  markerForTarget,
  newImageCount,
  newTargetImageCount,
  projectSeenKey,
  PROJECT_SEEN_STORAGE_KEY,
  saveProjectSeenState,
} from '../projectRecency';

const project: ProjectOverview & { db_id: string } = {
  id: 7,
  db_id: 'rig-a',
  profile_id: 'default',
  profile_name: 'Default',
  name: 'Flaming Star',
  display_name: 'Flaming Star',
  has_files: true,
  target_count: 1,
  total_images: 12,
  accepted_images: 8,
  rejected_images: 1,
  pending_images: 3,
  total_desired: 20,
  files_found: 12,
  files_missing: 0,
  date_range: { earliest: 100, latest: 200 },
  filters_used: ['Ha'],
  recent_images: [],
};

const target: TargetOverview & { db_id: string } = {
  id: 10,
  db_id: 'rig-a',
  project_id: 7,
  project_name: 'Flaming Star',
  name: 'IC 405',
  active: true,
  image_count: 12,
  accepted_count: 8,
  rejected_count: 1,
  pending_count: 3,
  total_desired: 20,
  files_found: 12,
  files_missing: 0,
  has_files: true,
  date_range: { earliest: 100, latest: 200 },
  filters_used: ['Ha'],
};

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => values.set(key, value),
  };
}

describe('project recency markers', () => {
  it('does not call an existing project new until a baseline exists', () => {
    expect(newImageCount(project, {})).toBe(0);
  });

  it('counts images added after the project was last opened', () => {
    const seen = {
      [projectSeenKey(project.db_id, project.id)]: {
        latestImage: 150,
        totalImages: 9,
        targets: {},
      },
    };

    expect(newImageCount(project, seen)).toBe(3);
  });

  it('flags a later image even when the row count stays unchanged', () => {
    const seen = {
      [projectSeenKey(project.db_id, project.id)]: {
        latestImage: 150,
        totalImages: 12,
        targets: {},
      },
    };

    expect(newImageCount(project, seen)).toBe(1);
  });

  it('tracks new images per target and rolls them up to the project', () => {
    const secondTarget = {
      ...target,
      id: 11,
      name: 'IC 410',
      image_count: 5,
      date_range: { earliest: 100, latest: 180 },
    };
    const seen = {
      [projectSeenKey(project.db_id, project.id)]: {
        latestImage: 150,
        totalImages: 14,
        targets: {
          [target.id]: { latestImage: 150, totalImages: 10 },
          [secondTarget.id]: markerForTarget(secondTarget),
        },
      },
    };

    expect(newTargetImageCount(target, seen)).toBe(2);
    expect(newTargetImageCount(secondTarget, seen)).toBe(0);
    expect(newImageCount(project, seen, [target, secondTarget])).toBe(2);
  });

  it('treats every image in a newly added target as new', () => {
    const seen = {
      [projectSeenKey(project.db_id, project.id)]: {
        latestImage: 150,
        totalImages: 9,
        targets: {
          other: { latestImage: 150, totalImages: 9 },
        },
      },
    };

    expect(newTargetImageCount(target, seen)).toBe(target.image_count);
  });

  it('loads only valid markers and survives bad browser data', () => {
    const storage = memoryStorage();
    storage.setItem(
      PROJECT_SEEN_STORAGE_KEY,
      JSON.stringify({
        valid: {
          latestImage: 10,
          totalImages: 2,
          targets: {
            10: { latestImage: 8, totalImages: 1 },
            bad: { latestImage: 'yesterday', totalImages: 1 },
          },
        },
        invalid: { latestImage: 'yesterday', totalImages: 2 },
      })
    );
    expect(loadProjectSeenState(storage)).toEqual({
      valid: {
        latestImage: 10,
        totalImages: 2,
        targets: {
          10: { latestImage: 8, totalImages: 1 },
        },
      },
    });

    storage.setItem(PROJECT_SEEN_STORAGE_KEY, '{bad json');
    expect(loadProjectSeenState(storage)).toEqual({});
  });

  it('saves the current project marker', () => {
    const storage = memoryStorage();
    const state = {
      [projectSeenKey(project.db_id, project.id)]: markerForProject(project, [target]),
    };
    saveProjectSeenState(state, storage);

    expect(loadProjectSeenState(storage)).toEqual(state);
  });
});
