import { describe, expect, it } from 'vitest';
import type { DatabaseSummary, Project, TargetNavigation } from '../../api/types';
import type { WithDb } from '../../hooks/useDatabases';
import { buildProjectTargetNavigation } from '../projectTargetNavigation';

const database: DatabaseSummary = {
  id: 'rig',
  name: 'Imaging Rig',
  database_path: '/tmp/rig.sqlite',
  image_directories: ['/tmp/images'],
  remote_image_upload: {
    enabled: false,
    token_configured: false,
    sync_enabled: false,
  },
};

const projects: WithDb<Project>[] = [
  {
    id: 1,
    profile_id: 'profile',
    profile_name: 'Profile',
    name: 'Project Alpha',
    display_name: 'Project Alpha',
    description: null,
    has_files: true,
    state: 1,
    latest_image_date: 100,
    db_id: database.id,
    db_name: database.name,
  },
  {
    id: 2,
    profile_id: 'profile',
    profile_name: 'Profile',
    name: 'Project Beta',
    display_name: 'Project Beta',
    description: null,
    has_files: true,
    state: 1,
    latest_image_date: 200,
    db_id: database.id,
    db_name: database.name,
  },
];

const targets: WithDb<TargetNavigation>[] = [
  {
    id: 10,
    project_id: 1,
    name: 'Alpha Field',
    active: true,
    has_files: true,
    db_id: database.id,
    db_name: database.name,
  },
  {
    id: 20,
    project_id: 2,
    name: 'Beta Field',
    active: true,
    has_files: true,
    db_id: database.id,
    db_name: database.name,
  },
];

describe('buildProjectTargetNavigation', () => {
  it('finds a project through a target name and marks it for initial expansion', () => {
    const model = buildProjectTargetNavigation({
      projects,
      targets,
      databases: [database],
      search: 'Beta Field',
      relativeNow: 1_000_000,
    });

    expect(model.projectGroups.flatMap((group) => group.projects).map((project) => project.id))
      .toEqual([2]);
    expect([...model.matchingTargetProjectKeys]).toEqual(['rig:2']);
    expect(model.targetsForProject(projects[1]).map((target) => target.id)).toEqual([20]);
  });

  it('does not mark a project-name match as a target-search expansion', () => {
    const model = buildProjectTargetNavigation({
      projects,
      targets,
      databases: [database],
      search: 'Project Alpha',
      relativeNow: 1_000_000,
    });

    expect([...model.matchingTargetProjectKeys]).toEqual([]);
    expect(model.targetsForProject(projects[0]).map((target) => target.id)).toEqual([10]);
  });
});
