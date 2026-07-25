import { describe, expect, it } from 'vitest';
import {
  groupProjectsByActivity,
  isArchivedProject,
  isRecentProject,
  projectMatchesSearch,
  sortProjects,
  type NavigableProject,
} from '../projectNavigation';

const NOW = Date.UTC(2026, 6, 25);

function project(
  id: number,
  name: string,
  ageDays: number | null,
  state = 1,
  totalImages = 0
): NavigableProject {
  return {
    id,
    db_id: 'demo',
    db_name: 'Demo catalog',
    name,
    display_name: name,
    state,
    latest_image_date:
      ageDays === null ? null : Math.floor(NOW / 1000 - ageDays * 24 * 60 * 60),
    total_images: totalImages,
  };
}

describe('project navigation', () => {
  it('groups projects by the last captured frame', () => {
    const groups = groupProjectsByActivity(
      [
        project(1, 'Today', 1),
        project(2, 'This month', 12),
        project(3, 'Old', 80),
        project(4, 'Unknown', null),
      ],
      NOW
    );

    expect(groups.map((group) => [group.id, group.projects[0].name])).toEqual([
      ['recent', 'Today'],
      ['month', 'This month'],
      ['earlier', 'Old'],
      ['undated', 'Unknown'],
    ]);
  });

  it('sorts by date, name, or image count', () => {
    const projects = [project(1, 'Zulu', 2, 1, 4), project(2, 'Alpha', 1, 1, 12)];
    expect(sortProjects(projects, 'recent').map(({ name }) => name)).toEqual(['Alpha', 'Zulu']);
    expect(sortProjects(projects, 'name').map(({ name }) => name)).toEqual(['Alpha', 'Zulu']);
    expect(sortProjects(projects, 'images').map(({ name }) => name)).toEqual(['Alpha', 'Zulu']);
  });

  it('finds projects by name or database and keeps closed projects archived', () => {
    const closed = project(1, 'Flaming Star', 1, 3);
    expect(isArchivedProject(closed)).toBe(true);
    expect(isRecentProject(closed, NOW)).toBe(true);
    expect(projectMatchesSearch(closed, 'flaming')).toBe(true);
    expect(projectMatchesSearch(closed, 'demo catalog')).toBe(true);
    expect(projectMatchesSearch(closed, 'veil')).toBe(false);
  });
});
