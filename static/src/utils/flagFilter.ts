import type { ImageQualityResult } from '../api/types';
import { formatCategory } from './issueCategory';

/**
 * The Images tab's Flag filter: one quality issue category, or `all`. The
 * value is the category as the API spells it (`rotation_skew`), which is
 * also what the URL carries.
 */
export const ALL_FLAGS = 'all';

export function parseFlagFilter(raw: string | null | undefined): string {
  const value = (raw ?? '').trim().toLowerCase();
  return value === '' || value === ALL_FLAGS ? ALL_FLAGS : value;
}

/** An image passes when no flag is chosen, or when its quality result
 * carries that flag. An image with no quality result never carries one. */
export function matchesFlagFilter(filter: string, quality: ImageQualityResult | undefined): boolean {
  const parsed = parseFlagFilter(filter);
  if (parsed === ALL_FLAGS) return true;
  return (quality?.flags ?? []).includes(parsed);
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
