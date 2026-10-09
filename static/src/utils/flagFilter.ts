import type { ImageQualityResult } from '../api/types';
import { formatCategory } from './issueCategory';

/**
 * The Images tab's Flag filter: quality issue categories to keep, none for
 * all. Each is the category as the API spells it (`rotation_skew`), which
 * is also what the URL carries.
 */
export const ALL_FLAGS = 'all';

export function parseFlagFilter(raw: string | null | undefined): string {
  const value = (raw ?? '').trim().toLowerCase();
  return value === '' || value === ALL_FLAGS ? ALL_FLAGS : value;
}

/** An image passes when no flag is chosen, or when its quality result
 * carries any chosen flag. An image with no quality result carries none. */
export function matchesFlagFilter(flags: readonly string[], quality: ImageQualityResult | undefined): boolean {
  if (flags.length === 0) return true;
  const carried = quality?.flags ?? [];
  return flags.some((flag) => carried.includes(parseFlagFilter(flag)));
}

/** Every flag any loaded quality result carries, sorted by label. */
export function availableFlags(results: Iterable<ImageQualityResult>): string[] {
  const flags = new Set<string>();
  for (const result of results) {
    for (const flag of result.flags ?? []) flags.add(flag);
  }
  return Array.from(flags).sort((left, right) =>
    formatCategory(left).localeCompare(formatCategory(right))
  );
}

export function flagFilterLabel(filter: string): string {
  const parsed = parseFlagFilter(filter);
  return parsed === ALL_FLAGS ? 'All' : formatCategory(parsed);
}
