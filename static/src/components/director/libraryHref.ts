import { withoutPlanningParams } from '../../hooks/useUrlState';

/** The plan list moved into the Library: an old Planning link lands there
 *  with its catalog scope, and its Show and search choices carried over. */
export function libraryHref(params: URLSearchParams): string {
  const next = withoutPlanningParams(params.toString());
  const show = next.get('directorShow');
  const search = next.get('directorSearch');
  next.delete('directorShow'); next.delete('directorSearch');
  if (show) next.set('show', show);
  if (search) next.set('q', search);
  const query = next.toString();
  return query ? `/?${query}` : '/';
}
