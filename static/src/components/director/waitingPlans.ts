import type { DirectorPlanRow } from '../../api/directorTypes';
import { waitingPlanMatchesShow, type LibraryShow } from '../libraryShow';
import { matchesSearch } from './planFilters';
import { CLOSED_PROJECT_STATE } from '../../utils/projectNavigation';

/** Plans with no project row in the Library. When a database's projects could
 *  not be read, `listed` is short and a linked plan would look empty, so only
 *  plans with no database are certain. With the Database filter on, only
 *  that database's row counts: the filter hides a row another rig has, so a
 *  plan that rig has frames for still waits here for this one. */
export function waitingPlans(rows: DirectorPlanRow[], listed: ReadonlySet<string>, incomplete: boolean, dbFilter: string | null = null): DirectorPlanRow[] {
  return rows.filter(row => {
    if (incomplete && row.links.length > 0) return false;
    const inFilter = dbFilter ? row.links.filter(link => link.catalog_slug === dbFilter) : [];
    const links = inFilter.length > 0 ? inFilter : row.links;
    return links.every(link => link.source_row_id === null || !listed.has(`${link.catalog_slug}:${link.source_row_id}`));
  });
}

/** The waiting plans the Library's search, Show and Database filters leave. */
export function shownWaitingPlans(waiting: DirectorPlanRow[], show: LibraryShow, search: string, dbFilter: string | null): DirectorPlanRow[] {
  return waiting.filter(row => waitingPlanMatchesShow(row.links, show, !!row.activation_due) && matchesSearch(row, search)
    && (!dbFilter || row.links.some(link => link.catalog_slug === dbFilter)));
}

/** The rigs where a plan has no project row in `listed`, as `slug:id`:
 *  activated there, with no frame yet. A rig where the project is Closed is
 *  left out. */
export function rigsWithoutFrames(row: DirectorPlanRow, listed: ReadonlySet<string>): DirectorPlanRow['links'] {
  return row.links.filter(link => link.source_state !== CLOSED_PROJECT_STATE
    && (link.source_row_id === null || !listed.has(`${link.catalog_slug}:${link.source_row_id}`)));
}
