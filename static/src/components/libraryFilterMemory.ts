/**
 * The Library's filters as last set in this browser tab: Database, Show,
 * the search and the order. They live in the URL, which the header carries
 * to the other views and back. A link that builds a fresh Library address
 * would drop them, so the Library also keeps them here. When it opens with
 * none in its URL, it puts these back. A cleared filter is kept as cleared,
 * so Show all stays all.
 */
export const LIBRARY_FILTER_KEYS = ['dbfilter', 'show', 'q', 'sort'] as const;

export type LibraryFilters = Partial<Record<typeof LIBRARY_FILTER_KEYS[number], string>>;

const KEY = 'psf-guard:library-filters:v1';

function storage(): Storage | null {
  try {
    return typeof window !== 'undefined' ? window.sessionStorage : null;
  } catch {
    return null;
  }
}

/** The filters a URL sets, without the empty ones. */
export function filtersOf(params: URLSearchParams): LibraryFilters {
  const filters: LibraryFilters = {};
  for (const key of LIBRARY_FILTER_KEYS) {
    const value = params.get(key);
    if (value) filters[key] = value;
  }
  return filters;
}

/** The filters last set in this tab, or none. */
export function recallLibraryFilters(): LibraryFilters {
  try {
    const raw = storage()?.getItem(KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    if (typeof parsed !== 'object' || parsed === null) return {};
    const filters: LibraryFilters = {};
    for (const key of LIBRARY_FILTER_KEYS) {
      const value = (parsed as Record<string, unknown>)[key];
      if (typeof value === 'string' && value) filters[key] = value;
    }
    return filters;
  } catch {
    return {};
  }
}

/** Keep these filters for the tab; none clears what was kept. */
export function rememberLibraryFilters(filters: LibraryFilters): void {
  try {
    storage()?.setItem(KEY, JSON.stringify(filters));
  } catch {
    // Storage may be full or refused; the URL still holds the filters.
  }
}
