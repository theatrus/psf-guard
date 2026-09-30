import { describe, expect, it } from 'vitest';
import type { DirectorPlanLink, DirectorPlanRow } from '../../api/directorTypes';
import { matchesSearch } from '../director/planFilters';
import { familyMatchesShow, isDoneProject, parseShow, waitingPlanMatchesShow } from '../libraryShow';
import { libraryHref } from '../director/libraryHref';

const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
const link = (extra: Partial<DirectorPlanLink> = {}): DirectorPlanLink => ({
  catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'g', source_row_id: 1, source_name: 'M31', source_state: 1,
  earliest_capture_s: null, latest_capture_s: null, targets: [], ...extra,
});
const plan = (links: DirectorPlanLink[], name = 'M31'): DirectorPlanRow => ({ project: { id: 'p', name, revision: 1 }, links, progress: null, framing: null, plan: null, activation: null });
const member = (state: number, accepted: number, desired: number) => ({ state, accepted_images: accepted, total_desired: desired });

describe('Library show filter', () => {
  it('matches a state on any rig, and Closed or Done only on every rig', () => {
    const mixed = [member(1, 10, 40), member(2, 40, 40)];
    expect(familyMatchesShow(mixed, 'active')).toBe(true);
    expect(familyMatchesShow(mixed, 'inactive')).toBe(true);
    expect(familyMatchesShow(mixed, 'draft')).toBe(false);
    expect(familyMatchesShow(mixed, 'done')).toBe(false);
    expect(familyMatchesShow(mixed, 'open')).toBe(true);
    expect(familyMatchesShow(mixed, 'closed')).toBe(false);
    const finished = [member(1, 40, 40), member(3, 25, 20)];
    expect(familyMatchesShow(finished, 'done')).toBe(true);
    expect(familyMatchesShow(finished, 'open')).toBe(false);
    expect(familyMatchesShow([member(3, 1, 4), member(3, 0, 4)], 'closed')).toBe(true);
    expect(familyMatchesShow([member(3, 1, 4), member(3, 0, 4)], 'open')).toBe(false);
    // A project in a database is never "no database".
    expect(familyMatchesShow(mixed, 'unlinked')).toBe(false);
  });

  it('keeps a waiting plan closed on every rig out of the list until Closed is asked for', () => {
    expect(waitingPlanMatchesShow([{ source_state: 3 }, { source_state: 3 }], 'all')).toBe(false);
    expect(waitingPlanMatchesShow([{ source_state: 3 }, { source_state: 3 }], 'closed')).toBe(true);
    expect(waitingPlanMatchesShow([{ source_state: 3 }, { source_state: 1 }], 'all')).toBe(true);
    expect(waitingPlanMatchesShow([], 'all')).toBe(true);
    expect(waitingPlanMatchesShow([], 'unlinked')).toBe(true);
    expect(waitingPlanMatchesShow([{ source_state: 1 }], 'done')).toBe(false);
  });

  it('treats a project without a goal as not done', () => {
    expect(isDoneProject(member(1, 5, 0))).toBe(false);
    expect(familyMatchesShow([member(1, 5, 0)], 'open')).toBe(true);
  });

  it('offers No database only with Planning, and falls back to all', () => {
    expect(parseShow('done', false)).toBe('done');
    expect(parseShow('unlinked', true)).toBe('unlinked');
    expect(parseShow('unlinked', false)).toBe('all');
    expect(parseShow('bogus', true)).toBe('all');
    expect(parseShow(null, true)).toBe('all');
  });
});

describe('plan search', () => {
  it('searches the plan name, the rig project name and the database', () => {
    const row = plan([link({ source_name: 'Andromeda subs', catalog_name: 'Redcat data' })]);
    expect(matchesSearch(row, 'andromeda')).toBe(true);
    expect(matchesSearch(row, 'redcat')).toBe(true);
    expect(matchesSearch(row, 'm31')).toBe(true);
    expect(matchesSearch(row, 'pelican')).toBe(false);
    expect(matchesSearch(row, '  ')).toBe(true);
  });
});

describe('old Planning links', () => {
  it('land in the Library with their scope, Show and search, and drop Planning-only params', () => {
    expect(libraryHref(new URLSearchParams('db=c925&project=3&directorView=projects&directorShow=done&directorSearch=m3&dbfilter=c925'))).toBe('/?db=c925&project=3&dbfilter=c925&show=done&q=m3');
    expect(libraryHref(new URLSearchParams(''))).toBe('/');
  });
});
