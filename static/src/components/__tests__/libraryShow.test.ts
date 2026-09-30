import { describe, expect, it } from 'vitest';
import type { DirectorPlanLink, DirectorPlanRow } from '../../api/directorTypes';
import { matchesSearch } from '../director/planFilters';
import { familyMatchesShow, isDoneProject, parseShow, waitingPlanMatchesShow } from '../libraryShow';
import { legacyPlanningHref, planHref, planKey, resolvePlan } from '../director/planAddress';

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

describe('plan addresses', () => {
  const shared = plan([link({ source_project_guid: 'AAAA-1' }), link({ catalog_slug: 'rc51', source_project_guid: 'aaaa-1', source_row_id: 3 })]);
  const attached = plan([link({ source_project_guid: 'g-1' }), link({ catalog_slug: 'rc51', source_project_guid: 'g-2', source_row_id: 3 })], 'Attached');
  attached.project.id = 'plan-attached';
  const bare = plan([], 'Bare');
  bare.project.id = 'plan-bare';

  it('names a plan by the GUID its rigs share, else by its plan id', () => {
    const rows = [shared, attached, bare];
    expect(planKey(shared, rows)).toBe('aaaa-1');
    expect(planKey(attached, rows)).toBe('plan-attached');
    expect(planKey(bare, rows)).toBe('plan-bare');
  });

  it('gives up the GUID when a detach leaves two plans holding it, and keeps the open one', () => {
    // Detaching rc51 keeps its GUID: two plans now carry aaaa-1.
    const kept = plan([link({ source_project_guid: 'aaaa-1' })], 'Kept');
    kept.project.id = 'plan-kept';
    const detached = plan([link({ catalog_slug: 'rc51', source_project_guid: 'AAAA-1', source_row_id: 3 })], 'Detached');
    detached.project.id = 'plan-detached';
    const rows = [kept, detached];
    expect(planKey(kept, rows)).toBe('plan-kept');
    expect(planKey(detached, rows)).toBe('plan-detached');
    expect(resolvePlan(rows, 'aaaa-1')).toEqual({ kind: 'ambiguous', rows: [kept, detached] });
    expect(resolvePlan(rows, 'aaaa-1', 'plan-detached')).toEqual({ kind: 'plan', row: detached });
    expect(resolvePlan(rows, 'rc51:3')).toEqual({ kind: 'plan', row: detached });
  });

  it('finds a plan by id, by any rig GUID, or by a database row; a row without a plan stays a row', () => {
    const rows = [shared, attached, bare];
    expect(resolvePlan(rows, 'AAAA-1')).toEqual({ kind: 'plan', row: shared });
    expect(resolvePlan(rows, 'g-2')).toEqual({ kind: 'plan', row: attached });
    expect(resolvePlan(rows, 'PLAN-BARE')).toEqual({ kind: 'plan', row: bare });
    expect(resolvePlan(rows, 'rc51:3')).toEqual({ kind: 'plan', row: shared });
    expect(resolvePlan(rows, 'c925:99')).toEqual({ kind: 'source', slug: 'c925', projectId: 99 });
    expect(resolvePlan(rows, 'nonsense')).toEqual({ kind: 'missing' });
    expect(resolvePlan(rows, null)).toEqual({ kind: 'missing' });
  });

  it('keeps the page scope and drops Planning-only params', () => {
    expect(planHref('aaaa-1', 'db=c925&project=3&show=done&directorView=projects&plan=old')).toBe('/plan?db=c925&project=3&show=done&plan=aaaa-1');
  });

  it('sends old Planning links to their plan, their row, or the Library with Show and search', () => {
    expect(legacyPlanningHref(new URLSearchParams('db=c925&project=3&directorProject=plan-bare&directorShow=done'))).toBe('/plan?db=c925&project=3&show=done&plan=plan-bare');
    expect(legacyPlanningHref(new URLSearchParams('db=c925&project=3&directorSource=c925&directorView=projects'))).toBe('/plan?db=c925&project=3&plan=c925%3A3');
    expect(legacyPlanningHref(new URLSearchParams('db=c925&project=3&directorView=projects&directorShow=done&directorSearch=m3&dbfilter=c925'))).toBe('/?db=c925&project=3&dbfilter=c925&show=done&q=m3');
    expect(legacyPlanningHref(new URLSearchParams(''))).toBe('/');
  });
});
