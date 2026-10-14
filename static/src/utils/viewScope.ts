import { withoutPlanningParams } from '../hooks/useUrlState';
import { imageDetailReturnView } from './imageDetailRoutes';

/**
 * URL keys that belong to one view rather than to the scope. Images and
 * Sequence each read `size` as their own thumbnail size, so it stays
 * behind when the header moves to another view, and that view's own saved
 * size applies there.
 */
export const VIEW_ONLY_PARAMS = ['size'] as const;

/** Which view's thumbnails a page shows: Images or Sequence, or the one a
 *  detail or comparison opened from. Other pages show none. */
function thumbnailViewOf(pathname: string, params: URLSearchParams): 'grid' | 'sequence' | null {
  if (pathname === '/grid') return 'grid';
  if (pathname === '/sequence') return 'sequence';
  if (pathname.startsWith('/detail/') || pathname.startsWith('/compare/')) {
    return imageDetailReturnView(params);
  }
  return null;
}

/**
 * The query the header carries to `toPath`: the scope (database, project,
 * target, filters, order, selection), less Planning's own keys and, when
 * the view changes, the keys that belong to the view being left.
 */
export function scopeForView(search: string, fromPath: string, toPath: string): URLSearchParams {
  const next = withoutPlanningParams(search);
  const from = thumbnailViewOf(fromPath, next);
  const to = thumbnailViewOf(toPath, next);
  if (from === null || from !== to) {
    for (const key of VIEW_ONLY_PARAMS) next.delete(key);
  }
  return next;
}
