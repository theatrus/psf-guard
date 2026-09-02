import { GradingStatus } from '../api/types';

/**
 * The Images tab's Status filter, as one word: what the select emits, what
 * the URL carries, and what the grid and keyboard navigation test against.
 *
 * There used to be two vocabularies — the select emitted the grade number
 * ("1") while the filter looked the value up by word ("accepted") — so any
 * choice but All matched nothing. Everything now goes through here.
 */
export type StatusFilter = 'all' | 'pending' | 'accepted' | 'rejected';

export const STATUS_FILTER_OPTIONS: ReadonlyArray<{ value: StatusFilter; label: string }> = [
  { value: 'all', label: 'All' },
  { value: 'accepted', label: 'Accepted' },
  { value: 'rejected', label: 'Rejected' },
  { value: 'pending', label: 'Pending' },
];

const GRADE_OF: Record<Exclude<StatusFilter, 'all'>, GradingStatus> = {
  pending: GradingStatus.Pending,
  accepted: GradingStatus.Accepted,
  rejected: GradingStatus.Rejected,
};

const WORD_OF_GRADE: Record<string, StatusFilter> = {
  [String(GradingStatus.Pending)]: 'pending',
  [String(GradingStatus.Accepted)]: 'accepted',
  [String(GradingStatus.Rejected)]: 'rejected',
};

/** Read a filter value from a select or the URL. The words are canonical;
 * the grade numbers are still accepted so links saved while the select
 * emitted them keep working. Anything else means no filter. */
export function parseStatusFilter(raw: string | null | undefined): StatusFilter {
  if (!raw) return 'all';
  const value = raw.trim().toLowerCase();
  if (value === 'all' || value in GRADE_OF) return value as StatusFilter;
  return WORD_OF_GRADE[value] ?? 'all';
}

export function matchesStatusFilter(filter: string, gradingStatus: number): boolean {
  const parsed = parseStatusFilter(filter);
  return parsed === 'all' || GRADE_OF[parsed] === gradingStatus;
}

export function statusFilterLabel(filter: string): string {
  const parsed = parseStatusFilter(filter);
  return STATUS_FILTER_OPTIONS.find((option) => option.value === parsed)?.label ?? 'All';
}
