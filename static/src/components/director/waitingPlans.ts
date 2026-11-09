import type { DirectorPlanRow } from '../../api/directorTypes';
import { waitingPlanMatchesShow, type LibraryShow } from '../libraryShow';
import { matchesSearch } from './planFilters';

/** Plans with no project row in the Library. When a database's projects could
 *  not be read, `listed` is short and a linked plan would look empty, so only
 *  plans with no database are certain. */
export function waitingPlans(rows: DirectorPlanRow[], listed: ReadonlySet<string>, incomplete: boolean): DirectorPlanRow[] {
  return rows.filter(row => (!incomplete || row.links.length === 0)
    && row.links.every(link => link.source_row_id === null || !listed.has(`${link.catalog_slug}:${link.source_row_id}`)));
}

/** The waiting plans the Library's search, Show and Database filters leave. */
export function shownWaitingPlans(waiting: DirectorPlanRow[], show: LibraryShow, search: string, dbFilter: string | null): DirectorPlanRow[] {
  return waiting.filter(row => waitingPlanMatchesShow(row.links, show, !!row.activation_due) && matchesSearch(row, search)
    && (!dbFilter || row.links.some(link => link.catalog_slug === dbFilter)));
}
