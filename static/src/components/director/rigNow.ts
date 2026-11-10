import type { DirectorRigStatusView, SyncLive } from '../../api/directorTypes';
import { describeNow, isExposing, statusIsStale } from './dashboardModel';
import { describeSequence, sequenceIsStale } from './syncLiveModel';

/** What a rig is doing now, from whichever plugin reports it. The Director
 *  plugin sends a status report; the Sync plugin sends N.I.N.A.'s sequence.
 *  A rig may run either plugin or both, so every place that states what a
 *  rig is doing reads it through here. */

/** A fresh Sync report of N.I.N.A.'s sequence taking an exposure. */
export function sequenceIsExposing(sync: SyncLive | null | undefined, nowMs: number): boolean {
  return !!sync?.sequence && !sequenceIsStale(sync, nowMs)
    && sync.sequence.running && sync.sequence.activity?.kind === 'exposing';
}

/** Exposing by either plugin's fresh report. */
export function rigIsExposing(view: DirectorRigStatusView, nowMs = Date.now()): boolean {
  return isExposing(view, nowMs) || sequenceIsExposing(view.sync, nowMs);
}

/** The Sync plugin's report speaks for the rig: it is fresh and the
 *  Director plugin's is missing or stale, or neither is fresh and there is
 *  no Director report at all. */
function syncSpeaks(view: DirectorRigStatusView, nowMs: number): boolean {
  const sync = view.sync;
  if (!sync?.sequence) return false;
  if (!view.status) return true;
  return statusIsStale(view, nowMs) && !sequenceIsStale(sync, nowMs);
}

/** One line for what the rig is doing, from the plugin whose report is
 *  current. The Live table shows both plugins' lines; other places show this. */
export function describeRigNow(view: DirectorRigStatusView, nowMs: number): string {
  return syncSpeaks(view, nowMs) ? describeSequence(view.sync, nowMs)! : describeNow(view, nowMs);
}

/** The target the rig reports working on, from the same plugin as
 *  `describeRigNow`, or null. */
export function rigTargetName(view: DirectorRigStatusView, nowMs: number): string | null {
  const name = (value: unknown) => typeof value === 'string' && value.trim() ? value.trim() : null;
  if (syncSpeaks(view, nowMs)) return name(view.sync?.sequence?.activity?.target_name);
  const payload = view.status?.payload ?? {};
  return name(payload.target_name) ?? name(payload.target);
}

/** The report `describeRigNow` reads is past its freshness window. */
export function rigReportIsStale(view: DirectorRigStatusView, nowMs: number): boolean {
  return syncSpeaks(view, nowMs) ? sequenceIsStale(view.sync!, nowMs) : statusIsStale(view, nowMs);
}
