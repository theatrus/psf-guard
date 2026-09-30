import type { DirectorRigStatusView } from '../../api/directorTypes';
import { describeNow } from '../director/dashboardModel';

export interface LiveSummary {
  /** "2 rigs · 1 exposing", or "2 rigs · 1 online" when none is exposing. */
  label: string;
  /** A rig that used to talk has gone quiet or offline. Never-seen rigs do not alarm. */
  alert: boolean;
  /** One line per rig for the tooltip. */
  title: string;
}

const phaseOf = (view: DirectorRigStatusView): string => {
  const p = view.status?.payload ?? {};
  const phase = typeof p.phase === 'string' ? p.phase : typeof p.state === 'string' ? p.state : '';
  return phase.toLowerCase();
};

/** Count the fleet the way the header chip states it. A rig is exposing when
 *  its last report says so and that report is not stale. */
export function liveSummary(rows: DirectorRigStatusView[], nowMs: number): LiveSummary {
  const rigs = rows.length;
  const exposing = rows.filter(view => !view.status_stale && /expos|imag|captur/.test(phaseOf(view))).length;
  const online = rows.filter(view => view.connectivity.state === 'online').length;
  const quiet = rows.filter(view => view.connectivity.state === 'stale' || view.connectivity.state === 'offline').length;
  const plural = rigs === 1 ? 'rig' : 'rigs';
  const label = exposing > 0 ? `${rigs} ${plural} · ${exposing} exposing` : `${rigs} ${plural} · ${online} online`;
  const title = rows.map(view => `${view.catalog_name ?? view.rig.name}: ${describeNow(view, nowMs)}`).join('\n');
  return { label, alert: quiet > 0, title };
}
