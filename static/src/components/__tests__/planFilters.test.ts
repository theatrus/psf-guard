import { describe, expect, it } from 'vitest';
import type { DirectorPlanLink, DirectorPlanRow } from '../../api/directorTypes';
import { isDonePlan, matchesFilter, matchesSearch, parsePlanFilter } from '../director/planFilters';

const rig = { id: '22222222-2222-4222-8222-222222222222', name: 'C925', revision: 1 };
const link = (state: number | null, desired: number, accepted: number, extra: Partial<DirectorPlanLink> = {}): DirectorPlanLink => ({
  catalog_slug: 'c925', catalog_name: 'C925', rig, source_project_guid: 'g', source_row_id: 1, source_name: 'M31', source_state: state,
  earliest_capture_s: null, latest_capture_s: null, targets: [{ name: 'M31', desired, acquired: accepted, accepted, rejected: 0, center: null, rotation_degrees: null }], ...extra,
});
const plan = (links: DirectorPlanLink[], name = 'M31'): DirectorPlanRow => ({ project: { id: 'p', name, revision: 1 }, links, progress: null, framing: null, plan: null, activation: null });

describe('plan filters', () => {
  it('matches a state on any rig, and Closed or Done only on every rig', () => {
    const mixed = plan([link(1, 40, 10), link(2, 40, 40)]);
    expect(matchesFilter(mixed, 'active')).toBe(true);
    expect(matchesFilter(mixed, 'inactive')).toBe(true);
    expect(matchesFilter(mixed, 'draft')).toBe(false);
    expect(matchesFilter(mixed, 'done')).toBe(false);
    expect(matchesFilter(mixed, 'open')).toBe(true);
    expect(matchesFilter(mixed, 'closed')).toBe(false);
    const finished = plan([link(1, 40, 40), link(3, 20, 25)]);
    expect(isDonePlan(finished)).toBe(true);
    expect(matchesFilter(finished, 'done')).toBe(true);
    expect(matchesFilter(finished, 'open')).toBe(false);
    expect(matchesFilter(plan([link(3, 40, 10), link(3, 40, 10)]), 'closed')).toBe(true);
  });

  it('treats a plan without a goal or a database as neither done nor open', () => {
    expect(isDonePlan(plan([link(1, 0, 5)]))).toBe(false);
    expect(matchesFilter(plan([]), 'unlinked')).toBe(true);
    expect(matchesFilter(plan([]), 'open')).toBe(false);
    expect(matchesFilter(plan([]), 'all')).toBe(true);
  });

  it('searches the plan name, the rig project name and the database', () => {
    const row = plan([link(1, 40, 10, { source_name: 'Andromeda subs', catalog_name: 'Redcat data' })]);
    expect(matchesSearch(row, 'andromeda')).toBe(true);
    expect(matchesSearch(row, 'redcat')).toBe(true);
    expect(matchesSearch(row, 'm31')).toBe(true);
    expect(matchesSearch(row, 'pelican')).toBe(false);
    expect(matchesSearch(row, '  ')).toBe(true);
  });

  it('falls back to all plans for an unknown filter', () => {
    expect(parsePlanFilter('done')).toBe('done');
    expect(parsePlanFilter('bogus')).toBe('all');
    expect(parsePlanFilter(null)).toBe('all');
  });
});
