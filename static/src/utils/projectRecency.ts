import type { ProjectOverview, TargetOverview } from '../api/types';

export const PROJECT_SEEN_STORAGE_KEY = 'psf-guard:project-seen:v2';

/**
 * What this browser had seen of a target or project when it was last
 * opened from the Library: its newest capture time and its frame count.
 * The Library's "N new" badges count from here.
 */
export interface TargetSeenMarker {
  latestImage: number;
  totalImages: number;
  /** The newest capture time seen before that visit. The frames captured
   *  after it were new when it opened, and Images and Sequence keep
   *  marking them until the next visit from the Library. */
  newSince?: number;
}

export interface ProjectSeenMarker extends TargetSeenMarker {
  targets: Record<string, TargetSeenMarker>;
}

export type ProjectSeenState = Record<string, ProjectSeenMarker>;

type ProjectRecency = Pick<ProjectOverview, 'id' | 'total_images' | 'date_range'> & {
  db_id: string;
};

type TargetRecency = Pick<TargetOverview, 'id' | 'project_id' | 'image_count' | 'date_range'> & {
  db_id: string;
};

export function projectSeenKey(dbId: string, projectId: number): string {
  return `${dbId}:${projectId}`;
}

export function markerForTarget(target: TargetRecency): TargetSeenMarker {
  return {
    latestImage: target.date_range.latest ?? 0,
    totalImages: target.image_count,
  };
}

export function markerForProject(
  project: ProjectRecency,
  targets: TargetRecency[] = []
): ProjectSeenMarker {
  return {
    latestImage: project.date_range.latest ?? 0,
    totalImages: project.total_images,
    targets: Object.fromEntries(
      targets.map((target) => [String(target.id), markerForTarget(target)])
    ),
  };
}

/** What was seen of one target before a visit: its own marker, or the
 *  project's when the target has none yet. */
function seenMarker(project: ProjectSeenMarker, targetId: number): TargetSeenMarker {
  return project.targets[String(targetId)] ?? project;
}

/** A project opened from the Library: everything in it seen now, and what
 *  was new when it opened kept as each marker's `newSince`. */
export function revisitProject(
  previous: ProjectSeenMarker | undefined,
  project: ProjectRecency,
  targets: TargetRecency[] = []
): ProjectSeenMarker {
  const next = markerForProject(project, targets);
  if (!previous) return next;
  return {
    ...next,
    newSince: previous.latestImage,
    targets: Object.fromEntries(
      Object.entries(next.targets).map(([id, marker]) => [
        id,
        { ...marker, newSince: seenMarker(previous, Number(id)).latestImage },
      ])
    ),
  };
}

/** A target opened from the Library, the same way. */
export function revisitTarget(
  previous: ProjectSeenMarker | undefined,
  target: TargetRecency
): TargetSeenMarker {
  const next = markerForTarget(target);
  return previous
    ? { ...next, newSince: seenMarker(previous, target.id).latestImage }
    : next;
}

/** A frame's identity as the new-frame rule needs it. */
export interface FrameRecency {
  project_id: number;
  target_id: number;
  acquired_date: number | null;
}

/**
 * Whether a frame is new: captured after the newest frame seen before the
 * latest visit to its target, or else its project, from the Library. These
 * are the frames the Library's "N new" badge counted when that visit
 * began, plus any captured since. A project this browser has never listed
 * has no new frames, as in the Library, so a first look marks nothing.
 */
export function isNewFrame(seen: ProjectSeenState, dbId: string, frame: FrameRecency): boolean {
  if (frame.acquired_date == null) return false;
  const project = seen[projectSeenKey(dbId, frame.project_id)];
  if (!project) return false;
  const marker = seenMarker(project, frame.target_id);
  const since = typeof marker.newSince === 'number' ? marker.newSince : marker.latestImage;
  return frame.acquired_date > since;
}

function countSince(marker: TargetSeenMarker, total: number, latest: number): number {
  const countGrowth = Math.max(0, total - marker.totalImages);
  const hasLaterImage = latest > marker.latestImage;
  return Math.max(countGrowth, hasLaterImage ? 1 : 0);
}

export function newTargetImageCount(
  target: TargetRecency,
  seen: ProjectSeenState
): number {
  const projectMarker = seen[projectSeenKey(target.db_id, target.project_id)];
  if (!projectMarker) return 0;

  const targetMarker = projectMarker.targets[String(target.id)];
  if (!targetMarker) return target.image_count;
  return countSince(
    targetMarker,
    target.image_count,
    target.date_range.latest ?? 0
  );
}

export function newImageCount(
  project: ProjectRecency,
  seen: ProjectSeenState,
  targets: TargetRecency[] = []
): number {
  const marker = seen[projectSeenKey(project.db_id, project.id)];
  if (!marker) return 0;

  if (targets.length > 0 && Object.keys(marker.targets).length > 0) {
    return targets.reduce(
      (total, target) => total + newTargetImageCount(target, seen),
      0
    );
  }

  return countSince(
    marker,
    project.total_images,
    project.date_range.latest ?? 0
  );
}

function isSeenMarker(value: unknown): value is TargetSeenMarker {
  return (
    !!value &&
    typeof value === 'object' &&
    typeof (value as TargetSeenMarker).latestImage === 'number' &&
    typeof (value as TargetSeenMarker).totalImages === 'number'
  );
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
      Object.entries(parsed).flatMap(([key, value]) => {
        if (!isSeenMarker(value)) return [];
        const rawTargets =
          'targets' in value &&
          value.targets &&
          typeof value.targets === 'object' &&
          !Array.isArray(value.targets)
            ? value.targets
            : {};
        const targets = Object.fromEntries(
          Object.entries(rawTargets).filter((entry): entry is [string, TargetSeenMarker] =>
            isSeenMarker(entry[1])
          )
        );
        return [[key, { ...value, targets }]];
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
