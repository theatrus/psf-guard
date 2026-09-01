/** Human label for a quality issue category. One shared mapping so the
 * grid badge, Sequence view, and detail panel tell the same story about
 * the same frame. */
export function formatCategory(category: string): string {
  if (category === 'satellite_trail_risk') return 'Satellite Trail Detected';
  if (category === 'hfr_above_limit') return 'HFR Above Limit';
  return category
    .split('_')
    .map(word => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ');
}
