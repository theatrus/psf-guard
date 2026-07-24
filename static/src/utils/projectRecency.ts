import type { ProjectOverview } from '../api/types';

export const PROJECT_SEEN_STORAGE_KEY = 'psf-guard:project-seen:v1';

export interface ProjectSeenMarker {
  latestImage: number;
  totalImages: number;
}

export type ProjectSeenState = Record<string, ProjectSeenMarker>;

type ProjectRecency = Pick<ProjectOverview, 'id' | 'total_images' | 'date_range'> & {
  db_id: string;
};

export function projectSeenKey(dbId: string, projectId: number): string {
  return `${dbId}:${projectId}`;
}

export function markerForProject(project: ProjectRecency): ProjectSeenMarker {
  return {
    latestImage: project.date_range.latest ?? 0,
    totalImages: project.total_images,
  };
}

export function newImageCount(
  project: ProjectRecency,
  seen: ProjectSeenState
): number {
  const marker = seen[projectSeenKey(project.db_id, project.id)];
  if (!marker) return 0;

  const countGrowth = Math.max(0, project.total_images - marker.totalImages);
  const hasLaterImage =
    (project.date_range.latest ?? 0) > marker.latestImage;
  return Math.max(countGrowth, hasLaterImage ? 1 : 0);
}

export function loadProjectSeenState(
  storage: Storage | undefined = typeof window === 'undefined'
    ? undefined
    : window.localStorage
): ProjectSeenState {
  if (!storage) return {};

  try {
    const parsed: unknown = JSON.parse(
      storage.getItem(PROJECT_SEEN_STORAGE_KEY) ?? '{}'
    );
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};

    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, ProjectSeenMarker] => {
        const marker = entry[1];
        return (
          !!marker &&
          typeof marker === 'object' &&
          typeof (marker as ProjectSeenMarker).latestImage === 'number' &&
          typeof (marker as ProjectSeenMarker).totalImages === 'number'
        );
      })
    );
  } catch {
    return {};
  }
}

export function saveProjectSeenState(
  state: ProjectSeenState,
  storage: Storage | undefined = typeof window === 'undefined'
    ? undefined
    : window.localStorage
): void {
  if (!storage) return;

  try {
    storage.setItem(PROJECT_SEEN_STORAGE_KEY, JSON.stringify(state));
  } catch {
    // A blocked or full browser store must not stop project navigation.
  }
}
