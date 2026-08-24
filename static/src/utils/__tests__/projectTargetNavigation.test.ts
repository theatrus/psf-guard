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

  it('groups one section per database when asked, in configured order', () => {
    const shed: DatabaseSummary = { ...database, id: 'shed', name: 'Shed catalog' };
    const shedProject: WithDb<Project> = {
      ...projects[0],
      id: 7,
      name: 'Shed survey',
      display_name: 'Shed survey',
      db_id: shed.id,
      db_name: shed.name,
    };

    const model = buildProjectTargetNavigation({
      projects: [...projects, shedProject],
      targets,
      databases: [database, shed],
      search: '',
      relativeNow: 1_000_000,
      grouping: 'database',
    });

    expect(model.projectGroups.map((group) => group.label)).toEqual([
      'Imaging Rig',
      'Shed catalog',
    ]);
    expect(model.projectGroups[0].projects.map((project) => project.id)).toEqual([2, 1]);
    expect(model.projectGroups[1].projects.map((project) => project.id)).toEqual([7]);
  });

  it('drops a database section that has nothing to show', () => {
    const empty: DatabaseSummary = { ...database, id: 'empty', name: 'Empty catalog' };
    const model = buildProjectTargetNavigation({
      projects,
      targets,
      databases: [database, empty],
      search: '',
      relativeNow: 1_000_000,
      grouping: 'database',
    });

    expect(model.projectGroups.map((group) => group.label)).toEqual(['Imaging Rig']);
  });
});
