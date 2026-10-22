import type {
  StackActivityEntry,
  StackReprocessing,
  StackStretchPendingProgress,
} from '../api/types';

/**
 * The line under a card that shows its previous stack's processed result
 * while the refreshed stack is processed again: what the re-apply is doing,
 * from its row in the background jobs when it has one.
 */
export function carriedLabel(
  reprocessing: StackReprocessing,
  run?: StackActivityEntry
): string {
  const lead = 'From the previous stack · reprocessing';
  if (reprocessing.state === 'failed') {
    return `${lead} failed${reprocessing.error ? `: ${reprocessing.error}` : ''}`;
  }
  if (run) {
    const status = run.state === 'queued' ? run.detail : run.progress_label ?? run.detail;
    return `${lead} · ${status}`;
  }
  const status = {
    waiting: 'waiting for the stack to settle',
    queued: 'queued',
    running: 'running',
  }[reprocessing.state];
  return `${lead} · ${status}`;
}

/**
 * The processing run a person asked for on one channel of a stack build,
 * while it waits or runs on the server, from the background jobs. A page
 * that was closed and opened again finds it here, since only the request
 * that started it follows it otherwise. Runs the server re-applies after a
 * refresh are left out: the card's reprocessing line shows those.
 */
export function processingRunFor(
  active: StackActivityEntry[],
  dbId: string,
  jobId: string,
  groupIndex: number
): StackActivityEntry | undefined {
  return active.find((entry) =>
    entry.kind === 'processing'
    && !entry.automatic
    && entry.database_id === dbId
    && entry.processing?.stack_job_id === jobId
    && entry.processing?.group_index === groupIndex
  );
}

/** A processing run's activity entry as the progress Apply processing shows. */
export function runProgress(entry: StackActivityEntry): StackStretchPendingProgress {
  const processing = entry.processing;
  if (entry.state === 'queued') {
    return {
      pending: true,
      queued: true,
      stage: entry.detail || undefined,
      waiting_for: processing?.waiting_for ?? undefined,
      queue_position: entry.queue_position ?? undefined,
    };
  }
  return {
    pending: true,
    stage: processing?.stage ?? undefined,
    fraction: processing?.fraction ?? undefined,
  };
}

/** A tool's name as people say it: "RC-Astro BlurXTerminator" is "BlurXTerminator". */
export function toolName(stage: string): string {
  return stage.replace(/^RC-Astro /, '');
}

/**
 * What Apply processing says while a run is in flight: what it waits for in
 * the stack worker's line, then the step under way and how far it has got.
 * Before the server has said anything it reads "Applying…".
 */
export function applyingLabel(progress: StackStretchPendingProgress | null): string {
  if (progress?.queued) {
    const ahead = progress.queue_position ? ` · ${progress.queue_position} ahead` : '';
    if (progress.waiting_for) return `Waiting for ${progress.waiting_for} to finish${ahead}`;
    return `${progress.stage ?? 'Queued'}${ahead}`;
  }
  if (progress?.fraction != null) {
    const applying = `Applying… ${Math.round(progress.fraction * 100)}%`;
    return progress.stage ? `${applying} · ${toolName(progress.stage)}` : applying;
  }
  if (progress?.stage) return `Applying… · ${toolName(progress.stage)}`;
  return 'Applying…';
}
