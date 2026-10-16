import type { FlatMeasureStatus } from '../../api/types';

/** The flat measuring job's stages, as the server names them. */
export const FLAT_STAGES = {
  measuring: 'measuring flat sessions',
  comparing: 'comparing flat sessions',
  evidence: 'reading lights shot during a change',
} as const;

const STAGE_WORDS: Record<string, { verb: string; unit: string }> = {
  [FLAT_STAGES.measuring]: { verb: 'Measuring flats', unit: 'session' },
  [FLAT_STAGES.comparing]: { verb: 'Comparing flats', unit: 'pair' },
  [FLAT_STAGES.evidence]: { verb: 'Reading lights shot during a change', unit: 'session' },
};

/** "Measuring flats · 12 of 40 sessions": what the job does now, in words. */
export function flatMeasureLabel(status: Pick<FlatMeasureStatus, 'stage' | 'done' | 'total'>): string {
  const words = STAGE_WORDS[status.stage ?? ''] ?? STAGE_WORDS[FLAT_STAGES.measuring];
  if (status.total <= 0) return words.verb;
  const unit = status.total === 1 ? words.unit : `${words.unit}s`;
  return `${words.verb} · ${Math.min(status.done, status.total)} of ${status.total} ${unit}`;
}

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

/** Whether Stop paused this rig's work, or any rig's when none is named. */
export function stoppedRig(status: FlatMeasureStatus | undefined, rig?: string): boolean {
  const stopped = status?.stopped ?? [];
  return stopped.length > 0 && (!rig || stopped.includes(rig) || stopped.includes('*'));
}

/** Whether the job is measuring this rig now, or any rig when none is named. */
export function measuringRig(status: FlatMeasureStatus | undefined, rig?: string): boolean {
  return !!status?.active && (!rig || status.rig_uuid === rig);
}
