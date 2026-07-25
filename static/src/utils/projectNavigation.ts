export const CLOSED_PROJECT_STATE = 3;
export const RECENT_PROJECT_SECONDS = 7 * 24 * 60 * 60;
const MONTH_SECONDS = 30 * 24 * 60 * 60;

export type ProjectActivityGroupId = 'recent' | 'month' | 'earlier' | 'undated';
export type ProjectSort = 'recent' | 'name' | 'images';

export interface NavigableProject {
  id: number;
  db_id: string;
  db_name: string;
  display_name: string;
  name: string;
  state?: number;
  latest_image_date?: number | null;
  date_range?: { latest?: number };
  total_images?: number;
}

export interface ProjectActivityGroup<T extends NavigableProject> {
  id: ProjectActivityGroupId;
  label: string;
  projects: T[];
}

const ACTIVITY_GROUPS: Array<{ id: ProjectActivityGroupId; label: string }> = [
  { id: 'recent', label: 'Worked on this week' },
  { id: 'month', label: 'Worked on this month' },
  { id: 'earlier', label: 'Earlier work' },
  { id: 'undated', label: 'Date unknown' },
];

export function projectLastWorkedAt(project: NavigableProject): number | null {
  return project.latest_image_date ?? project.date_range?.latest ?? null;
}

export function isArchivedProject(project: NavigableProject): boolean {
  return project.state === CLOSED_PROJECT_STATE;
}

export function isRecentProject(
  project: NavigableProject,
  nowMilliseconds = Date.now()
): boolean {
  const latest = projectLastWorkedAt(project);
  if (latest === null) return false;
  const age = nowMilliseconds / 1000 - latest;
  return age >= 0 && age <= RECENT_PROJECT_SECONDS;
}

export function projectMatchesSearch(project: NavigableProject, search: string): boolean {
  const query = search.trim().toLocaleLowerCase();
  if (!query) return true;
  return [
    project.display_name,
    project.name,
    project.db_name,
    project.db_id,
  ].some((value) => value.toLocaleLowerCase().includes(query));
}

export function sortProjects<T extends NavigableProject>(
  projects: T[],
  sort: ProjectSort
): T[] {
  return [...projects].sort((left, right) => {
    if (sort === 'name') {
      return (
        left.display_name.localeCompare(right.display_name) ||
        left.db_name.localeCompare(right.db_name) ||
        left.id - right.id
      );
    }
    if (sort === 'images') {
      const imageDifference = (right.total_images ?? 0) - (left.total_images ?? 0);
      if (imageDifference !== 0) return imageDifference;
    }
    const dateDifference =
      (projectLastWorkedAt(right) ?? Number.NEGATIVE_INFINITY) -
      (projectLastWorkedAt(left) ?? Number.NEGATIVE_INFINITY);
    return (
      dateDifference ||
      left.display_name.localeCompare(right.display_name) ||
      left.db_name.localeCompare(right.db_name) ||
      left.id - right.id
    );
  });
}

export function groupProjectsByActivity<T extends NavigableProject>(
  projects: T[],
  nowMilliseconds = Date.now(),
  sort: ProjectSort = 'recent'
): ProjectActivityGroup<T>[] {
  const nowSeconds = nowMilliseconds / 1000;
  const buckets = new Map<ProjectActivityGroupId, T[]>(
    ACTIVITY_GROUPS.map(({ id }) => [id, []])
  );

  for (const project of projects) {
    const latest = projectLastWorkedAt(project);
    let id: ProjectActivityGroupId = 'undated';
    if (latest !== null) {
      const age = Math.max(0, nowSeconds - latest);
      id = age <= RECENT_PROJECT_SECONDS ? 'recent' : age <= MONTH_SECONDS ? 'month' : 'earlier';
    }
    buckets.get(id)?.push(project);
  }

  return ACTIVITY_GROUPS.map(({ id, label }) => ({
    id,
    label,
    projects: sortProjects(buckets.get(id) ?? [], sort),
  })).filter((group) => group.projects.length > 0);
}
