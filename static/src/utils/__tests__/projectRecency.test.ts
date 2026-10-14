import { describe, expect, it } from 'vitest';
import type { ProjectOverview, TargetOverview } from '../../api/types';
import {
  isNewFrame,
  loadProjectSeenState,
  markerForProject,
  markerForTarget,
  newImageCount,
  newTargetImageCount,
  projectSeenKey,
  PROJECT_SEEN_STORAGE_KEY,
  revisitProject,
  revisitTarget,
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
  state: 1,
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

describe('new frames in Images and Sequence', () => {
  const frame = (acquired: number | null, targetId = target.id) => ({
    project_id: project.id,
    target_id: targetId,
    acquired_date: acquired,
  });
  const key = projectSeenKey(project.db_id, project.id);

  it('marks nothing in a project this browser has never listed', () => {
    expect(isNewFrame({}, project.db_id, frame(999))).toBe(false);
  });

  it('marks the frames captured after the newest one seen', () => {
    const seen = { [key]: markerForProject(project, [target]) };
    expect(isNewFrame(seen, project.db_id, frame(200))).toBe(false);
    expect(isNewFrame(seen, project.db_id, frame(201))).toBe(true);
    expect(isNewFrame(seen, project.db_id, frame(null))).toBe(false);
    // Another database's project of the same ID is not this one.
    expect(isNewFrame(seen, 'rig-b', frame(201))).toBe(false);
  });

  it('keeps the frames the Library counted as new marked through the visit that cleared them', () => {
    const before = { [key]: markerForProject(project, [target]) };
    // Three frames arrive, and the Library counts them.
    const grown = {
      ...project,
      total_images: 15,
      date_range: { earliest: 100, latest: 260 },
    };
    const grownTarget = { ...target, image_count: 15, date_range: { earliest: 100, latest: 260 } };
    expect(newImageCount(grown, before, [grownTarget])).toBe(3);

    // Opening the project clears its badge...
    const visit = { [key]: revisitProject(before[key], grown, [grownTarget]) };
    expect(newImageCount(grown, visit, [grownTarget])).toBe(0);
    // ...and the frames it counted stay new in the grid it opens.
    expect(isNewFrame(visit, project.db_id, frame(230))).toBe(true);
    expect(isNewFrame(visit, project.db_id, frame(200))).toBe(false);
    // A frame captured during the visit is new as well.
    expect(isNewFrame(visit, project.db_id, frame(300))).toBe(true);

    // The next visit starts from what this one saw.
    const again = { [key]: revisitProject(visit[key], grown, [grownTarget]) };
    expect(isNewFrame(again, project.db_id, frame(230))).toBe(false);
    expect(isNewFrame(again, project.db_id, frame(300))).toBe(true);
  });

  it('counts a target from the project when the target has no marker of its own', () => {
    const seen = { [key]: markerForProject(project, [target]) };
    // A target this browser never listed: frames after the project's
    // newest are new, its older back catalog is not.
    expect(isNewFrame(seen, project.db_id, frame(150, 11))).toBe(false);
    expect(isNewFrame(seen, project.db_id, frame(250, 11))).toBe(true);
  });

  it('opens one target the same way', () => {
    const before = markerForProject(project, [target]);
    const grownTarget = { ...target, image_count: 13, date_range: { earliest: 100, latest: 240 } };
    const marker = revisitTarget(before, grownTarget);
    expect(marker).toEqual({ latestImage: 240, totalImages: 13, newSince: 200 });
    const seen = { [key]: { ...before, targets: { [String(target.id)]: marker } } };
    expect(isNewFrame(seen, project.db_id, frame(240))).toBe(true);
    // With nothing seen before, opening marks nothing new.
    expect(revisitTarget(undefined, grownTarget)).toEqual(markerForTarget(grownTarget));
  });

  it('survives a round trip through storage', () => {
    const storage = memoryStorage();
    const visit = {
      [key]: revisitProject(markerForProject(project, [target]), {
        ...project,
        date_range: { earliest: 100, latest: 260 },
      }, [{ ...target, date_range: { earliest: 100, latest: 260 } }]),
    };
    saveProjectSeenState(visit, storage);
    expect(isNewFrame(loadProjectSeenState(storage), project.db_id, frame(230))).toBe(true);
  });
});
