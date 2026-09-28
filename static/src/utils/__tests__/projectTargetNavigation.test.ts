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
    directory_template: '%YEAR%/%TARGET%/%NIGHT%/%TYPE%',
    directory_template_source: 'preset',
    directory_template_samples: 0,
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

  describe('a project shot by several rigs', () => {
    const shed: DatabaseSummary = { ...database, id: 'shed', name: 'Shed catalog' };
    const guid = '9A1D4C2E-0000-4000-8000-000000000001';
    const rigHeart: WithDb<Project> = { ...projects[0], id: 5, name: 'Heart', display_name: 'Heart', guid, latest_image_date: 300 };
    const shedHeart: WithDb<Project> = { ...rigHeart, id: 8, guid: guid.toLowerCase(), latest_image_date: 250, db_id: shed.id, db_name: shed.name };
    const shedTarget: WithDb<TargetNavigation> = { id: 80, project_id: 8, name: 'Heart r1c2', active: true, has_files: true, db_id: shed.id, db_name: shed.name };
    const input = { projects: [...projects, rigHeart, shedHeart], targets: [...targets, shedTarget], databases: [database, shed], relativeNow: 1_000_000 };

    it('is one row by activity, led by the rig with the newest work', () => {
      const model = buildProjectTargetNavigation({ ...input, search: '' });
      const shown = model.projectGroups.flatMap((group) => group.projects);
      expect(shown.map((project) => `${project.db_id}:${project.id}`)).toEqual(['rig:5', 'rig:2', 'rig:1']);
      const family = model.familyOf(rigHeart);
      expect(family?.key).toBe(`guid:${guid.toLowerCase()}`);
      expect(family?.members.map((member) => member.db_id)).toEqual(['rig', 'shed']);
      expect(model.familyOf(shedHeart)).toBe(family);
      expect(model.familyOf(projects[0])).toBeNull();
    });

    it('keeps one row per rig when grouped by database', () => {
      const model = buildProjectTargetNavigation({ ...input, search: '', grouping: 'database' });
      expect(model.projectGroups.map((group) => group.projects.map((project) => project.id))).toEqual([[5, 2, 1], [8]]);
    });

    it('is found by any rig, its database or its targets, and opens to the matching rig', () => {
      const byOtherRig = buildProjectTargetNavigation({ ...input, search: 'shed' });
      expect(byOtherRig.projectGroups.flatMap((group) => group.projects).map((project) => project.id)).toEqual([5]);
      const byTarget = buildProjectTargetNavigation({ ...input, search: 'r1c2' });
      expect(byTarget.projectGroups.flatMap((group) => group.projects).map((project) => project.id)).toEqual([5]);
      expect([...byTarget.matchingTargetProjectKeys]).toEqual(['shed:8', `guid:${guid.toLowerCase()}`]);
      expect(byTarget.targetsForProject(shedHeart).map((target) => target.id)).toEqual([80]);
    });

    it('is one project only when the GUID spans databases', () => {
      const twin: WithDb<Project> = { ...rigHeart, id: 6 };
      const model = buildProjectTargetNavigation({ ...input, projects: [...projects, rigHeart, twin], search: '' });
      expect(model.familyOf(rigHeart)).toBeNull();
      expect(model.projectGroups.flatMap((group) => group.projects).map((project) => project.id)).toEqual([5, 6, 2, 1]);
    });
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
