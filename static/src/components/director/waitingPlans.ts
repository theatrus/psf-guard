import type { DirectorPlanLink, DirectorPlanRow } from '../../api/directorTypes';
import { waitingPlanMatchesShow, type LibraryShow } from '../libraryShow';
import { matchesSearch } from './planFilters';
import { CLOSED_PROJECT_STATE } from '../../utils/projectNavigation';

/** The rig has no project row in `listed` (as `slug:id`) for this link. A
 *  link whose database the server could not read is not known to be empty. */
const hasNoRow = (link: DirectorPlanLink, listed: ReadonlySet<string>) => !link.source_unread
  && (link.source_row_id === null || !listed.has(`${link.catalog_slug}:${link.source_row_id}`));

/** The links a view judges a plan by: the filtered database's when the
 *  Database filter is on, otherwise every rig's. */
export const linksInScope = (row: DirectorPlanRow, dbFilter: string | null): DirectorPlanLink[] =>
  dbFilter ? row.links.filter(link => link.catalog_slug === dbFilter) : row.links;

/** Plans with no project row in the Library. When a database's projects could
 *  not be read, `listed` is short and a linked plan would look empty, so only
 *  plans with no database are certain; the same goes for a rig the server
 *  could not read. With the Database filter on, only that database's row
 *  counts: the filter hides a row another rig has, so a plan that rig has
 *  frames for still waits here for this one. */
export function waitingPlans(rows: DirectorPlanRow[], listed: ReadonlySet<string>, incomplete: boolean, dbFilter: string | null = null): DirectorPlanRow[] {
  return rows.filter(row => {
    if (row.links.length === 0) return true;
    if (incomplete) return false;
    const scoped = linksInScope(row, dbFilter);
    const links = scoped.length > 0 ? scoped : row.links;
    return links.every(link => hasNoRow(link, listed));
  });
}

/** The waiting plans the Library's search, Show and Database filters leave.
 *  With the Database filter on, Show reads that database's project state, as
 *  it does for the Library's project rows. */
export function shownWaitingPlans(waiting: DirectorPlanRow[], show: LibraryShow, search: string, dbFilter: string | null): DirectorPlanRow[] {
  return waiting.filter(row => {
    const scoped = linksInScope(row, dbFilter);
    if (dbFilter && scoped.length === 0) return false;
    return waitingPlanMatchesShow(scoped, show, !!row.activation_due) && matchesSearch(row, search);
  });
}

/** The rigs where a plan was activated and has no frame yet. A rig where
 *  the project is Closed, or whose database the server could not read, is
 *  left out. */
export function rigsWithoutFrames(row: DirectorPlanRow, listed: ReadonlySet<string>): DirectorPlanLink[] {
  return row.links.filter(link => link.source_state !== CLOSED_PROJECT_STATE && hasNoRow(link, listed));
}

/** What the header's project picker offers below its projects, which are
 *  the ones with frames: plans on no rig, and plans activated on a rig with
 *  no frames yet, each with those rigs. When a database could not be read,
 *  only plans on no rig are certain. */
export function plansWaitingForFrames(rows: DirectorPlanRow[], listed: ReadonlySet<string>, incomplete: boolean, search: string): Array<{ row: DirectorPlanRow; rigs: DirectorPlanLink[] }> {
  return rows
    .filter(row => matchesSearch(row, search))
    .map(row => ({ row, rigs: row.links.length === 0 || incomplete ? [] : rigsWithoutFrames(row, listed) }))
    .filter(({ row, rigs }) => row.links.length === 0 || rigs.length > 0);
}
