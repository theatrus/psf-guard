import { parseStatusFilter } from './statusFilter';

/** The Images filters as the URL holds them. */
export interface ImageFilterState {
  status: string;
  filterNames: string[];
  dateRange: {
    start: string | null;
    end: string | null;
  };
  searchTerm: string;
  flags?: string[];
  calibration?: string[];
  /** One flat group's lights; see utils/flatGroupFilter. */
  flatGroup?: string | null;
}

/** How many Images filters narrow the grid; a date range counts once. */
export function imageFilterCount(filters: ImageFilterState): number {
  return Number(parseStatusFilter(filters.status) !== 'all')
    + Number(filters.filterNames.length > 0)
    + Number(Boolean(filters.dateRange.start || filters.dateRange.end))
    + Number(filters.searchTerm !== '')
    + Number((filters.flags ?? []).length > 0)
    + Number((filters.calibration ?? []).length > 0)
    + Number(Boolean(filters.flatGroup));
}
