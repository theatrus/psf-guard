import { keepScoringInScope, withoutPlanningParams } from '../../hooks/useUrlState';

/**
 * The calibration page for one database, from wherever a person is. The
 * scope comes along; a project from another database does not. Given a
 * project, the page shows the calibration that project's lights use,
 * unless `scoped` is false: a link to frames a light matched or was refused
 * points outside what it uses, so it opens the rig's whole library.
 */
export function calibrationPageHref(
  search: string,
  dbId: string,
  projectId?: number,
  { scoped = true }: { scoped?: boolean } = {},
): string {
  const params = withoutPlanningParams(search);
  if (params.get('db') !== dbId) {
    for (const key of ['project', 'target', 'mosaic']) params.delete(key);
  }
  params.set('db', dbId);
  if (projectId !== undefined) {
    params.set('project', String(projectId));
    if (scoped) params.set('calProject', String(projectId));
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
  return lightsGridHref(search, dbId, projectId, { calib, filterNames, dates: night });
}

/** Grid filters a link to some of a project's lights sets. */
export interface LightsGridFilters {
  /** A calibration level or finding code (utils/calibrationFilter). */
  calib?: string;
  filterNames?: readonly string[];
  dates?: NightDates;
}

/** The Images grid for one project's lights, narrowed by `filters`, with
 *  every other grid filter cleared so it shows exactly them. */
export function lightsGridHref(
  search: string,
  dbId: string,
  projectId: number,
  { calib, filterNames = [], dates }: LightsGridFilters,
): string {
  const params = withoutPlanningParams(search);
  for (const key of GRID_FILTER_KEYS) params.delete(key);
  params.set('db', dbId);
  params.set('project', String(projectId));
  if (calib) params.set('calib', calib);
  // Each value encoded on its own, as the grid's list filters write them.
  if (filterNames.length > 0) params.set('filter', filterNames.map(encodeURIComponent).join(','));
  if (dates) {
    params.set('dateStart', dates.dateStart);
    params.set('dateEnd', dates.dateEnd);
  }
  return `/grid?${keepScoringInScope(search, params)}`;
}

/** Slack either side of a span of lights' times: a light's catalog time
 *  and its header's can differ by a moment. */
const SPAN_SLACK_MS = 15 * 60 * 1000;

/** The grid's date filter for lights shot from `first` to `last`, Unix seconds. */
export function spanDates(first: number, last: number): NightDates {
  return {
    dateStart: new Date(first * 1000 - SPAN_SLACK_MS).toISOString(),
    dateEnd: new Date(last * 1000 + SPAN_SLACK_MS).toISOString(),
  };
}

/** One master in the Calibration page's Masters list. */
export function calibrationMasterHref(search: string, dbId: string, masterId: string): string {
  const [path, query] = calibrationPageHref(search, dbId).split('?');
  const params = new URLSearchParams(query);
  params.set('calTab', 'masters');
  params.set('calMaster', masterId);
  return `${path}?${params}`;
}
