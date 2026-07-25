const MINUTE_SECONDS = 60;
const HOUR_SECONDS = 60 * MINUTE_SECONDS;
const DAY_SECONDS = 24 * HOUR_SECONDS;
const WEEK_SECONDS = 7 * DAY_SECONDS;
const MONTH_SECONDS = 30 * DAY_SECONDS;
const YEAR_SECONDS = 365 * DAY_SECONDS;

interface RelativeUnit {
  seconds: number;
  singular: string;
  plural: string;
}

const RELATIVE_UNITS: RelativeUnit[] = [
  { seconds: YEAR_SECONDS, singular: 'yr', plural: 'yrs' },
  { seconds: MONTH_SECONDS, singular: 'mo', plural: 'mos' },
  { seconds: WEEK_SECONDS, singular: 'wk', plural: 'wks' },
  { seconds: DAY_SECONDS, singular: 'day', plural: 'days' },
  { seconds: HOUR_SECONDS, singular: 'hr', plural: 'hrs' },
  { seconds: MINUTE_SECONDS, singular: 'min', plural: 'mins' },
];

/**
 * Compact relative age for overview cards. The optional clock keeps tests
 * deterministic and lets callers update every visible age in one render.
 */
export function formatRelativeTime(
  timestampSeconds: number | null | undefined,
  nowMilliseconds = Date.now()
): string {
  if (timestampSeconds === null || timestampSeconds === undefined) {
    return 'Time unknown';
  }

  const differenceSeconds = Math.trunc(nowMilliseconds / 1000 - timestampSeconds);
  const absoluteSeconds = Math.abs(differenceSeconds);
  if (absoluteSeconds < MINUTE_SECONDS) return 'just now';

  const unit =
    RELATIVE_UNITS.find((candidate) => absoluteSeconds >= candidate.seconds) ??
    RELATIVE_UNITS[RELATIVE_UNITS.length - 1];
  const value = Math.floor(absoluteSeconds / unit.seconds);
  const label = value === 1 ? unit.singular : unit.plural;
  return differenceSeconds < 0
    ? `in ${value} ${label}`
    : `${value} ${label} ago`;
}
