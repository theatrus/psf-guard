import type { DirectorRigSummary, SyncLive } from '../../api/directorTypes';
import { describeNow, statusIsStale } from './dashboardModel';
import { describeSequence, sequenceIsStale } from './syncLiveModel';

/** What a rig is doing now, from whichever plugin reports it. The Director
 *  plugin sends a status report; the Sync plugin sends N.I.N.A.'s sequence.
 *  The server decides which report speaks (`report`) and whether the rig is
 *  exposing; these read that answer and word it. A page keeps showing the
 *  last answer while the server cannot be reached, so each also drops a
 *  report that has gone stale since. */

const syncSpeaks = (view: DirectorRigSummary) => view.report === 'sync' && !!view.sync?.sequence;

/** The report `describeRigNow` reads is past its freshness window. */
export function rigReportIsStale(view: DirectorRigSummary, nowMs: number): boolean {
  return syncSpeaks(view) ? sequenceIsStale(view.sync as SyncLive, nowMs) : statusIsStale(view, nowMs);
}

/** A fresh report from either plugin says the rig is exposing. */
export function rigIsExposing(view: DirectorRigSummary, nowMs = Date.now()): boolean {
  return view.exposing && !rigReportIsStale(view, nowMs);
}

/** One line for what the rig is doing, from the plugin whose report speaks.
 *  The Live table shows both plugins' lines; other places show this. */
export function describeRigNow(view: DirectorRigSummary, nowMs: number): string {
  return syncSpeaks(view) ? describeSequence(view.sync, nowMs)! : describeNow(view, nowMs);
}

/** The target the rig reports working on, from the same plugin as
 *  `describeRigNow`, or null. */
export function rigTargetName(view: DirectorRigSummary): string | null {
  const name = (value: unknown) => typeof value === 'string' && value.trim() ? value.trim() : null;
  if (syncSpeaks(view)) return name(view.sync?.sequence?.activity?.target_name);
  const payload = view.status?.payload ?? {};
  return name(payload.target_name) ?? name(payload.target);
}
