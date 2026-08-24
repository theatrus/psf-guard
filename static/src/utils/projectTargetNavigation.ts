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

interface NavigationModelInput {
  projects: NavigationProject[];
  targets: NavigationTarget[];
  databases: DatabaseSummary[];
  search: string;
  relativeNow: number;
  /** Narrows the whole tree to one database; null shows every catalog. */
  dbFilter?: string | null;
}

export function buildProjectTargetNavigation({
  projects,
  targets,
  databases,
  search,
  relativeNow,
  dbFilter = null,
}: NavigationModelInput) {
  const normalizedSearch = search.trim().toLocaleLowerCase();
  if (dbFilter) {
    projects = projects.filter((project) => project.db_id === dbFilter);
    databases = databases.filter((database) => database.id === dbFilter);
  }
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

  return {
    normalizedSearch,
    projectGroups: groupProjectsByActivity(
      matchingProjects.filter((project) => !isArchivedProject(project)),
      relativeNow
    ),
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
