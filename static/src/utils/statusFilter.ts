import { GradingStatus } from '../api/types';

/**
 * The Images tab's Status filter: which grades to show. One string for the
 * checkboxes, the URL and what the grid and keyboard navigation test
 * against: `all`, or the chosen grades as words joined by commas in one
 * fixed order (`accepted,pending` keeps everything but rejected).
 *
 * There used to be two vocabularies — the select emitted the grade number
 * ("1") while the filter looked the value up by word ("accepted") — so any
 * choice but All matched nothing. Everything now goes through here.
 */
export type StatusWord = 'accepted' | 'rejected' | 'pending';
export type StatusFilter = 'all' | string;

export const STATUS_FILTER_OPTIONS: ReadonlyArray<{ value: StatusWord; label: string }> = [
  { value: 'accepted', label: 'Accepted' },
  { value: 'rejected', label: 'Rejected' },
  { value: 'pending', label: 'Pending' },
];

const WORDS: readonly StatusWord[] = STATUS_FILTER_OPTIONS.map((option) => option.value);

const GRADE_OF: Record<StatusWord, GradingStatus> = {
  pending: GradingStatus.Pending,
  accepted: GradingStatus.Accepted,
  rejected: GradingStatus.Rejected,
};

const WORD_OF_GRADE: Record<string, StatusWord> = {
  [String(GradingStatus.Pending)]: 'pending',
  [String(GradingStatus.Accepted)]: 'accepted',
  [String(GradingStatus.Rejected)]: 'rejected',
};

/** The grades a filter keeps, in the checkboxes' order; every grade for
 * All. The words are canonical; the grade numbers are still read so links
 * saved while the select emitted them keep working. A filter naming no
 * grade it knows keeps them all. */
export function statusWords(raw: string | null | undefined): StatusWord[] {
  if (!raw) return [...WORDS];
  const chosen = new Set<StatusWord>();
  for (const part of raw.split(',')) {
    const value = part.trim().toLowerCase();
    if (value === 'all') return [...WORDS];
    const word = value in GRADE_OF ? (value as StatusWord) : WORD_OF_GRADE[value];
    if (word) chosen.add(word);
  }
  return chosen.size === 0 ? [...WORDS] : WORDS.filter((word) => chosen.has(word));
}

/** The filter that keeps these grades: `all` for every grade, or none. */
export function statusFilterOf(words: Iterable<StatusWord>): StatusFilter {
  const chosen = new Set(words);
  const kept = WORDS.filter((word) => chosen.has(word));
  return kept.length === 0 || kept.length === WORDS.length ? 'all' : kept.join(',');
}

/** Read a filter value from the checkboxes or the URL, in canonical form. */
export function parseStatusFilter(raw: string | null | undefined): StatusFilter {
  return statusFilterOf(statusWords(raw));
}

export function matchesStatusFilter(filter: string, gradingStatus: number): boolean {
  return statusWords(filter).some((word) => GRADE_OF[word] === gradingStatus);
}

export function statusFilterLabel(filter: string): string {
  const words = statusWords(filter);
  if (words.length === WORDS.length) return 'All';
  return words
    .map((word) => STATUS_FILTER_OPTIONS.find((option) => option.value === word)!.label)
    .join(' and ');
}
