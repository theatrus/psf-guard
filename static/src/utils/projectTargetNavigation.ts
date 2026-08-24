import type { DatabaseSummary, Project, TargetNavigation } from '../api/types';
import type { WithDb } from '../hooks/useDatabases';
import {
  groupProjectsByActivity,
  isArchivedProject,
  projectMatchesSearch,
  sortProjects,
} from './projectNavigation';

export type NavigationProject = WithDb<Project>;
export type NavigationTarget = WithDb<TargetNavigation>;

export function projectNavigationKey(project: NavigationProject): string {
  return `${project.db_id}:${project.id}`;
}

/** How the picker organizes its project list. */
export type NavigationGrouping = 'activity' | 'database';

interface NavigationModelInput {
  projects: NavigationProject[];
  targets: NavigationTarget[];
  databases: DatabaseSummary[];
  search: string;
  relativeNow: number;
  /** Group projects by recent activity (default) or one group per catalog. */
  grouping?: NavigationGrouping;
}

export function buildProjectTargetNavigation({
  projects,
  targets,
  databases,
  search,
  relativeNow,
  grouping = 'activity',
}: NavigationModelInput) {
  const normalizedSearch = search.trim().toLocaleLowerCase();
  const targetsByProject = new Map<string, NavigationTarget[]>();
  for (const target of targets) {
    const key = `${target.db_id}:${target.project_id}`;
    const items = targetsByProject.get(key) ?? [];
    items.push(target);
    targetsByProject.set(key, items);
  }
  for (const items of targetsByProject.values()) {
    items.sort((left, right) => left.name.localeCompare(right.name));
  }

  const targetMatchesSearch = (target: NavigationTarget) =>
    !normalizedSearch || target.name.toLocaleLowerCase().includes(normalizedSearch);
  const projectTargets = (project: NavigationProject) =>
    targetsByProject.get(projectNavigationKey(project)) ?? [];
  const targetsForProject = (project: NavigationProject) => {
    const allTargets = projectTargets(project);
    return projectMatchesSearch(project, search)
      ? allTargets
      : allTargets.filter(targetMatchesSearch);
  };

  const matchingProjects = projects.filter((project) => {
    if (projectMatchesSearch(project, search)) return true;
    return projectTargets(project).some(targetMatchesSearch);
  });
  const matchingTargetProjectKeys = new Set(
    matchingProjects
      .filter((project) => normalizedSearch && projectTargets(project).some(targetMatchesSearch))
      .map(projectNavigationKey)
  );

  const activeProjects = matchingProjects.filter((project) => !isArchivedProject(project));
  // Database grouping keeps the catalogs in their configured order and skips
  // the ones with nothing to show. Archived projects stay in the shared
  // section below either way; every row there already names its database.
  const projectGroups =
    grouping === 'database'
      ? databases
          .map((database) => ({
            id: `db:${database.id}`,
            label: database.name,
            projects: sortProjects(
              activeProjects.filter((project) => project.db_id === database.id),
              'recent'
            ),
          }))
          .filter((group) => group.projects.length > 0)
      : groupProjectsByActivity(activeProjects, relativeNow);

  return {
    normalizedSearch,
    projectGroups,
    archivedProjects: sortProjects(matchingProjects.filter(isArchivedProject), 'recent'),
    matchingDatabases: databases.filter(
      (database) =>
        !normalizedSearch ||
        database.name.toLocaleLowerCase().includes(normalizedSearch) ||
        database.id.toLocaleLowerCase().includes(normalizedSearch)
    ),
    matchingTargetProjectKeys,
    targetsForProject,
  };
}
