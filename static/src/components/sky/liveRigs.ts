import type { DirectorPlanRow, DirectorRigStatusView } from '../../api/directorTypes';
import { describeNow, isExposing, statusIsStale } from '../director/dashboardModel';

/** A rig as the Sky draws it: where it points now, if that can be told. */
export interface PlacedRig {
  id: string;
  name: string;
  /** Plugin report says it is exposing, and the report is fresh. */
  exposing: boolean;
  /** Drawn faded: the rig stopped talking (`quiet`), or it talks but its
   *  last status report is over ten minutes old (`old report`). */
  stale: boolean;
  staleReason: 'quiet' | 'old report' | null;
  connectivity: DirectorRigStatusView['connectivity']['state'];
  /** What the rig says it is doing, as the Live table words it. */
  now: string;
  /** ICRS degrees, or null when neither a pointing nor a known target is reported. */
  ra: number | null;
  dec: number | null;
  /** Where the place came from: the mount's own pointing (a report), or the
   *  centre of the target it names (an inference from the plan). */
  source: 'pointing' | 'target' | null;
  targetName: string | null;
  /** Why a rig is not drawn, when it is not. */
  unplaced: string | null;
}

const text = (value: unknown): string | null => typeof value === 'string' && value.trim() ? value.trim() : null;
const finite = (value: unknown): number | null => typeof value === 'number' && Number.isFinite(value) ? value : null;

/** The mount's pointing from the plugin's `pointing` field, ICRS degrees. */
function pointingOf(payload: Record<string, unknown>): { ra: number; dec: number } | null {
  const pointing = payload.pointing;
  if (!pointing || typeof pointing !== 'object') return null;
  const ra = finite((pointing as Record<string, unknown>).ra_degrees);
  const dec = finite((pointing as Record<string, unknown>).dec_degrees);
  if (ra === null || dec === null || dec < -90 || dec > 90) return null;
  return { ra: ((ra % 360) + 360) % 360, dec };
}

/** Place every reporting rig. The plugin's `pointing` wins; otherwise the
 *  target it names (`target_name`, or `target`) is looked up among the
 *  targets of that rig's own plan links, and its centre is used. A rig with
 *  neither is listed but not drawn. */
export function placeRigs(views: DirectorRigStatusView[], plans: DirectorPlanRow[], nowMs: number): PlacedRig[] {
  return views.map(view => {
    const payload = view.status?.payload ?? {};
    const targetName = text(payload.target_name) ?? text(payload.target);
    const quiet = view.connectivity.state === 'stale' || view.connectivity.state === 'offline';
    const staleReason = quiet ? 'quiet' as const : statusIsStale(view, nowMs) ? 'old report' as const : null;
    const base = {
      id: view.rig.id,
      name: view.catalog_name ?? view.rig.name,
      exposing: isExposing(view, nowMs),
      stale: staleReason !== null,
      staleReason,
      connectivity: view.connectivity.state,
      now: describeNow(view, nowMs),
      targetName,
    };
    const pointing = pointingOf(payload);
    if (pointing) return { ...base, ra: pointing.ra, dec: pointing.dec, source: 'pointing' as const, unplaced: null };
    if (!targetName) return { ...base, ra: null, dec: null, source: null, unplaced: view.status ? 'Reports no pointing and no target' : 'No report yet' };
    // The first of the rig's own plan targets with that name and a centre.
    const wanted = targetName.toLowerCase();
    for (const row of plans) for (const link of row.links) {
      if (!view.catalog_slug || link.catalog_slug !== view.catalog_slug) continue;
      const target = link.targets.find(entry => entry.name.toLowerCase() === wanted && entry.center);
      if (target?.center) return { ...base, ra: target.center.ra_degrees, dec: target.center.dec_degrees, source: 'target' as const, unplaced: null };
    }
    return { ...base, ra: null, dec: null, source: null, unplaced: `No place known for ${targetName}` };
  });
}
