import { describe, expect, it } from 'vitest';
import type { ProjectOverview } from '../../api/types';
import {
  loadProjectSeenState,
  markerForProject,
  newImageCount,
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
      },
    };

    expect(newImageCount(project, seen)).toBe(3);
  });

  it('flags a later image even when the row count stays unchanged', () => {
    const seen = {
      [projectSeenKey(project.db_id, project.id)]: {
        latestImage: 150,
        totalImages: 12,
      },
    };

    expect(newImageCount(project, seen)).toBe(1);
  });

  it('loads only valid markers and survives bad browser data', () => {
    const storage = memoryStorage();
    storage.setItem(
      PROJECT_SEEN_STORAGE_KEY,
      JSON.stringify({
        valid: { latestImage: 10, totalImages: 2 },
        invalid: { latestImage: 'yesterday', totalImages: 2 },
      })
    );
    expect(loadProjectSeenState(storage)).toEqual({
      valid: { latestImage: 10, totalImages: 2 },
    });

    storage.setItem(PROJECT_SEEN_STORAGE_KEY, '{bad json');
    expect(loadProjectSeenState(storage)).toEqual({});
  });

  it('saves the current project marker', () => {
    const storage = memoryStorage();
    const state = {
      [projectSeenKey(project.db_id, project.id)]: markerForProject(project),
    };
    saveProjectSeenState(state, storage);

    expect(loadProjectSeenState(storage)).toEqual(state);
  });
});
