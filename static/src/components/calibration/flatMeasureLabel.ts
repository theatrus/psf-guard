import type { FlatMeasureStatus } from '../../api/types';

/** The flat measuring job's stages, as the server names them. */
export const FLAT_STAGES = {
  measuring: 'measuring flat sessions',
  comparing: 'comparing flat sessions',
  evidence: 'reading lights shot during a change',
} as const;

/** One poll per catalog, shared by the header and the page: fast while a
 *  pass runs, slower while work only waits, and a slow heartbeat at rest,
 *  since the server starts the job on its own after an import or a sync. */
export const FLAT_MEASURE_POLL_MS = { running: 2_000, queued: 5_000, idle: 10_000 };

/** How often a page showing a rig tells the server so; well inside the
 *  server's 45-second watch. */
export const FLAT_WATCH_MS = 20_000;

export const flatMeasureKey = (dbId: string) => ['db', dbId, 'calibrations', 'flat-measure'] as const;

export function flatMeasurePollMs(status: FlatMeasureStatus | undefined): number {
  if (status?.state === 'running') return FLAT_MEASURE_POLL_MS.running;
  if (status?.state === 'queued') return FLAT_MEASURE_POLL_MS.queued;
  return FLAT_MEASURE_POLL_MS.idle;
}

/** Whether a person paused flat measuring. */
export function measuringPaused(status: FlatMeasureStatus | undefined): boolean {
  return status?.state === 'paused';
}

/** Whether the job is measuring this rig now, or any rig when none is named. */
export function measuringRig(status: FlatMeasureStatus | undefined, rig?: string): boolean {
  return !!status?.active && (!rig || status.rig_uuid === rig);
}
