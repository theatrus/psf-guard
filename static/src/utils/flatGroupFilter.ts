/**
 * The Images grid's flat-group filter: the lights whose flat master one
 * flat group's flats build, exactly as the Calibration page counts them.
 * The URL key is `flatGroup`, valued `<db>:<group key>`, or
 * `<db>:<group key>:flagged` for those of them flagged for their flats
 * (shot during a change with no side known, or given flats from the far
 * side of one). The database rides in the value so the filter names the
 * catalog it was made in and matches nothing in another.
 */
export const FLAT_GROUP_PARAM = 'flatGroup';

export interface FlatGroupFilter {
  dbId: string;
  /** The group's key, as the flat-groups view gives it. */
  key: string;
  /** Only the lights flagged for their flats. */
  flagged: boolean;
}

export function flatGroupFilterValue(filter: FlatGroupFilter): string {
  return `${filter.dbId}:${filter.key}${filter.flagged ? ':flagged' : ''}`;
}

/** The filter a `flatGroup` value names, or none for an empty or malformed one. */
export function parseFlatGroupFilter(value: string | null | undefined): FlatGroupFilter | null {
  if (!value) return null;
  const parts = value.split(':');
  if (parts.length < 2 || parts.length > 3 || !parts[0] || !parts[1]) return null;
  if (parts.length === 3 && parts[2] !== 'flagged') return null;
  return { dbId: parts[0], key: parts[1], flagged: parts[2] === 'flagged' };
}
