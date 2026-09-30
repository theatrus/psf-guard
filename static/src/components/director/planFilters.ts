import type { DirectorPlanRow } from '../../api/directorTypes';
import { isArchivedPlan, sumFrames } from './planCardModel';

/** What the plan list can narrow to. A plan shot by several rigs matches a
 *  state when any rig has it, since the plan is live wherever one rig still
 *  shoots it; Closed and Done ask for every rig. */
export type PlanFilter = 'all' | 'active' | 'open' | 'done' | 'draft' | 'inactive' | 'closed' | 'unlinked';

export const PLAN_FILTERS: ReadonlyArray<{ value: PlanFilter; label: string }> = [
  { value: 'all', label: 'All plans' },
  { value: 'active', label: 'Active' },
  { value: 'open', label: 'Still to shoot' },
  { value: 'done', label: 'Done' },
  { value: 'draft', label: 'Draft' },
  { value: 'inactive', label: 'Inactive' },
  { value: 'closed', label: 'Closed' },
  { value: 'unlinked', label: 'No database' },
];

export function parsePlanFilter(value: string | null): PlanFilter {
  return PLAN_FILTERS.some(filter => filter.value === value) ? value as PlanFilter : 'all';
}

/** Every rig that shoots the plan has met its goal. */
export function isDonePlan(row: DirectorPlanRow): boolean {
  return row.links.length > 0 && row.links.every(link => {
    const sum = sumFrames(link.targets);
    return sum.desired > 0 && sum.accepted >= sum.desired;
  });
}

const hasState = (row: DirectorPlanRow, state: number) => row.links.some(link => link.source_state === state);

export function matchesFilter(row: DirectorPlanRow, filter: PlanFilter): boolean {
  switch (filter) {
    case 'all': return true;
    case 'active': return hasState(row, 1);
    case 'inactive': return hasState(row, 2);
    case 'draft': return hasState(row, 0);
    case 'closed': return isArchivedPlan(row);
    case 'done': return isDonePlan(row);
    case 'open': return row.links.length > 0 && !isDonePlan(row) && !isArchivedPlan(row);
    case 'unlinked': return row.links.length === 0;
  }
}

/** The plan's name, or a rig's name for its project or database. */
export function matchesSearch(row: DirectorPlanRow, search: string): boolean {
  const needle = search.trim().toLowerCase();
  if (!needle) return true;
  const haystack = [row.project.name, ...row.links.flatMap(link => [link.source_name ?? '', link.catalog_name])];
  return haystack.some(text => text.toLowerCase().includes(needle));
}
