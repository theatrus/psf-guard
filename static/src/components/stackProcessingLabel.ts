import type { StackStretchPendingProgress } from '../api/types';

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
