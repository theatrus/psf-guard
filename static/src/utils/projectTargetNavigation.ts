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

/** One project that lives in several databases: Sync copies sharing a Target
 *  Scheduler GUID, each shot by its own rig. Director treats them as one plan
 *  and the picker shows them as one row. */
export interface NavigationFamily {
  key: string;
  guid: string;
  display_name: string;
  /** One per database, newest work first. */
  members: NavigationProject[];
}

export function familyNavigationKey(guid: string): string {
  return `guid:${guid.trim().toLocaleLowerCase()}`;
}

function buildFamilies(projects: NavigationProject[]): Map<string, NavigationFamily> {
  const byGuid = new Map<string, NavigationProject[]>();
  for (const project of projects) {
    const guid = project.guid?.trim().toLocaleLowerCase();
    if (!guid) continue;
    const members = byGuid.get(guid) ?? [];
    members.push(project);
    byGuid.set(guid, members);
  }
  const byProject = new Map<string, NavigationFamily>();
  for (const [guid, members] of byGuid) {
    if (new Set(members.map((member) => member.db_id)).size < 2) continue;
    const sorted = sortProjects(members, 'recent');
    const family: NavigationFamily = {
      key: familyNavigationKey(guid),
      guid,
      display_name: sorted[0].display_name,
      members: sorted,
    };
    for (const member of members) byProject.set(projectNavigationKey(member), family);
  }
  return byProject;
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
  const familyByProject = buildFamilies(projects);
  const familyOf = (project: NavigationProject) =>
    familyByProject.get(projectNavigationKey(project)) ?? null;
  // A family matches when any of its rigs does, by project name, database or
  // target: typing one rig's name must still find the shared project.
  const kinOf = (project: NavigationProject) => familyOf(project)?.members ?? [project];
  const projectOrKinMatches = (project: NavigationProject) =>
    kinOf(project).some((member) => projectMatchesSearch(member, search));
  const targetsForProject = (project: NavigationProject) => {
    const allTargets = projectTargets(project);
    return projectOrKinMatches(project) ? allTargets : allTargets.filter(targetMatchesSearch);
  };

  const matchingProjects = projects.filter(
    (project) =>
      projectOrKinMatches(project) ||
      kinOf(project).some((member) => projectTargets(member).some(targetMatchesSearch))
  );
  const matchingTargetProjectKeys = new Set<string>();
  for (const project of matchingProjects) {
    if (!normalizedSearch || !projectTargets(project).some(targetMatchesSearch)) continue;
    matchingTargetProjectKeys.add(projectNavigationKey(project));
    const family = familyOf(project);
    if (family) matchingTargetProjectKeys.add(family.key);
  }

  // Grouped by activity, a family is one row placed by its newest active
  // rig; the other rigs show inside it. Grouped by database, every rig keeps
  // its own row under its catalog, since that is the point of that view.
  const matching = new Set(matchingProjects.map(projectNavigationKey));
  const isFamilyLead = (project: NavigationProject) => {
    const family = familyOf(project);
    if (!family || grouping === 'database') return true;
    const present = family.members.filter((member) => matching.has(projectNavigationKey(member)));
    const lead = present.find((member) => !isArchivedProject(member)) ?? present[0];
    return lead === project;
  };
  const shownProjects = matchingProjects.filter(isFamilyLead);
  const activeProjects = shownProjects.filter((project) => !isArchivedProject(project));
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
    archivedProjects: sortProjects(shownProjects.filter(isArchivedProject), 'recent'),
    matchingDatabases: databases.filter(
      (database) =>
        !normalizedSearch ||
        database.name.toLocaleLowerCase().includes(normalizedSearch) ||
        database.id.toLocaleLowerCase().includes(normalizedSearch)
    ),
    matchingTargetProjectKeys,
    targetsForProject,
    familyOf,
  };
}
