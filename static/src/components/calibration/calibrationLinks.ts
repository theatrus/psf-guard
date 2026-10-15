import { keepScoringInScope, withoutPlanningParams } from '../../hooks/useUrlState';

/**
 * The calibration page for one database, from wherever a person is. The
 * scope comes along; a project from another database does not. Given a
 * project, the page shows the calibration that project's lights use.
 */
export function calibrationPageHref(search: string, dbId: string, projectId?: number): string {
  const params = withoutPlanningParams(search);
  if (params.get('db') !== dbId) {
    for (const key of ['project', 'target', 'mosaic']) params.delete(key);
  }
  params.set('db', dbId);
  if (projectId !== undefined) {
    params.set('project', String(projectId));
    params.set('calProject', String(projectId));
  }
  return `/calibration?${keepScoringInScope(search, params)}`;
}

/** One night's lights, as the grid's date filter reads them. */
export interface NightDates {
  dateStart: string;
  dateEnd: string;
}

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * The span of one night (`YYYY-MM-DD`) in a catalog whose nights split
 * `boundarySeconds` after 00:00 UTC, as the server counts lights by night:
 * from that boundary on the night's date to the instant before the next.
 * The grid keeps a light when `new Date(acquired_date * 1000)` lies within
 * `new Date(dateStart)` and `new Date(dateEnd)`, so full timestamps make it
 * count exactly the night's lights. None for a night that is not a date.
 */
export function nightDates(night: string, boundarySeconds: number): NightDates | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(night);
  if (!match) return undefined;
  const start = Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3])) + boundarySeconds * 1000;
  if (!Number.isFinite(start)) return undefined;
  return {
    dateStart: new Date(start).toISOString(),
    dateEnd: new Date(start + DAY_MS - 1).toISOString(),
  };
}

/** Grid keys a link to some lights clears, so the grid shows exactly them. */
const GRID_FILTER_KEYS = ['target', 'mosaic', 'status', 'filter', 'flag', 'calib', 'dateStart', 'dateEnd', 'search', 'selected', 'current', 'expanded'];

/**
 * The Images grid for one project's lights that a calibration level or
 * finding code covers (see utils/calibrationFilter), in the given filters
 * or all of them, and in one night or all of them.
 */
export function calibrationGridHref(
  search: string,
  dbId: string,
  projectId: number,
  calib: string,
  filterNames: readonly string[] = [],
  night?: NightDates,
): string {
  const params = withoutPlanningParams(search);
  for (const key of GRID_FILTER_KEYS) params.delete(key);
  params.set('db', dbId);
  params.set('project', String(projectId));
  params.set('calib', calib);
  // Each value encoded on its own, as the grid's list filters write them.
  if (filterNames.length > 0) params.set('filter', filterNames.map(encodeURIComponent).join(','));
  if (night) {
    params.set('dateStart', night.dateStart);
    params.set('dateEnd', night.dateEnd);
  }
  return `/grid?${keepScoringInScope(search, params)}`;
}

/** One master in the Calibration page's Masters list. */
export function calibrationMasterHref(search: string, dbId: string, masterId: string): string {
  const [path, query] = calibrationPageHref(search, dbId).split('?');
  const params = new URLSearchParams(query);
  params.set('calTab', 'masters');
  params.set('calMaster', masterId);
  return `${path}?${params}`;
}
