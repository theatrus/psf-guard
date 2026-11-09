import type { SyncLatestAction, SyncLive, SyncSequenceActivity } from '../../api/directorTypes';
import { formatAge } from './dashboardModel';

const SUBJECT: Record<string, string> = { merge: 'catalog', push_planning: 'planning', push_grades: 'grades' };
const subject = (operation?: string) => SUBJECT[operation ?? ''] ?? 'catalog';
const count = (summary: Record<string, number> | undefined, key: string) => summary?.[key] ?? 0;

/** "Merged N.I.N.A.'s catalog: 4 updated, 2 added", "Uploaded M31_0001.fits". */
export function describeSyncAction(action: SyncLatestAction): string {
  const verb = (() => {
    switch (action.action) {
      case 'export': return `Sent ${subject(action.operation)} to N.I.N.A.`;
      case 'preview': return `Checked N.I.N.A.'s ${subject(action.operation)}`;
      case 'preview_refresh': return `Checked N.I.N.A.'s ${subject(action.operation)} again`;
      case 'apply': return action.operation === 'merge' || !action.operation ? "Merged N.I.N.A.'s catalog" : `Applied N.I.N.A.'s ${subject(action.operation)}`;
      case 'pair': return 'Paired';
      case 'flat_history_snapshot': return 'Sent flat history';
      case 'flat_history_acknowledge': return 'Confirmed flat history';
      case 'star_detection_profile': return 'Sent star detection settings';
      case 'image_upload': return action.outcome === 'ok' && action.detail ? `Uploaded ${action.detail}` : 'Uploaded an image';
      default: return action.action.replaceAll('_', ' ');
    }
  })();
  if (action.outcome !== 'ok') {
    const result = action.outcome === 'refused' ? 'refused' : 'failed';
    return action.detail ? `${verb}: ${result} (${action.detail})` : `${verb}: ${result}`;
  }
  if (action.action === 'apply') {
    const parts = [
      count(action.summary, 'total_updated') && `${count(action.summary, 'total_updated')} updated`,
      count(action.summary, 'total_inserted') && `${count(action.summary, 'total_inserted')} added`,
    ].filter(Boolean);
    return parts.length ? `${verb}: ${parts.join(', ')}` : `${verb}: no changes`;
  }
  if (action.action === 'image_upload' && count(action.summary, 'already_present')) return `${verb} (already here)`;
  return verb;
}

const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
const seconds = (value: number) => `${Number.isInteger(value) ? value : value.toFixed(1)} s`;

/** One running instruction in plain words: "Exposing M 31: 300 s L, 3 of 30". */
export function describeActivity(activity: SyncSequenceActivity): string {
  const at = activity.target_name ? ` ${activity.target_name}` : '';
  switch (activity.kind) {
    case 'exposing': {
      const detail = [
        activity.exposure_seconds !== undefined ? seconds(activity.exposure_seconds) : null,
        activity.filter ?? null,
      ].filter(Boolean).join(' ');
      const progress = activity.completed !== undefined && activity.total ? `${activity.completed} of ${activity.total}` : null;
      const tail = [detail || null, progress].filter(Boolean).join(', ');
      return `Exposing${at}${tail ? `: ${tail}` : ''}`;
    }
    case 'autofocus': return 'Running autofocus';
    case 'dithering': return 'Dithering';
    case 'meridian_flip': return 'Meridian flip';
    case 'mount_slew': return `Slewing${activity.target_name ? ` to ${activity.target_name}` : ''}`;
    case 'mount_center': return `Centering${at}`;
    case 'plate_solve': return 'Plate solving';
    case 'camera_cooling': return activity.target_temperature_c !== undefined ? `Cooling camera to ${activity.target_temperature_c} °C` : 'Cooling camera';
    case 'camera_warming': return 'Warming camera';
    case 'time_wait': return activity.until_ms ? `Waiting until ${clock(activity.until_ms)}` : 'Waiting';
    case 'astronomical_wait': return activity.until_ms ? `Waiting for altitude, until about ${clock(activity.until_ms)}` : 'Waiting for altitude';
    case 'safety_wait': return 'Waiting until safe';
    case 'condition_wait': return 'Waiting for a condition';
    case 'manual_wait': return 'Paused until someone resumes it';
    case 'guiding': return 'Starting guiding';
    default: return activity.name ? `Running ${activity.name}` : 'Running an instruction';
  }
}

/** What N.I.N.A.'s sequence is doing, as the Sync plugin last reported it,
 *  or null without a report. A stale report says when it arrived. */
export function describeSequence(sync: SyncLive | null | undefined, nowMs: number): string | null {
  const sequence = sync?.sequence;
  if (!sequence) return null;
  const stale = sync.sequence_stale || nowMs - sequence.received_at_ms > sequence.fresh_for_ms;
  let line: string;
  if (!sequence.running) line = 'Sequence stopped';
  else if (sequence.activity) {
    line = describeActivity(sequence.activity);
    const started = sequence.activity.started_ms;
    if (started !== undefined && !stale) {
      // Plugin clock for the span it saw, server clock since it arrived.
      const elapsed = Math.max(0, sequence.reported_at_ms - started) + Math.max(0, nowMs - sequence.received_at_ms);
      line += ` for ${formatAge(elapsed).replace(' ago', '')}`;
    }
  } else line = sequence.operation ? `Running ${sequence.operation}` : 'Sequence running';
  return stale ? `${line} (stale, reported ${formatAge(nowMs - sequence.received_at_ms)})` : line;
}

/** "last frame 2 min ago · autofocus 40 min ago", from the plugin's clock. */
export function describeRecent(sync: SyncLive | null | undefined, nowMs: number): string | null {
  const sequence = sync?.sequence;
  if (!sequence) return null;
  // Shift plugin times onto the server clock by the report's own delay.
  const skew = sequence.received_at_ms - sequence.reported_at_ms;
  const ago = (ms: number | undefined) => ms === undefined ? null : formatAge(Math.max(0, nowMs - (ms + skew)));
  const parts = [
    ['last frame', ago(sequence.recent.image_saved_ms)],
    ['dither', ago(sequence.recent.dither_ms)],
    ['autofocus', ago(sequence.recent.autofocus_ms)],
    ['meridian flip', ago(sequence.recent.meridian_flip_ms)],
  ].filter((part): part is [string, string] => part[1] !== null).map(([label, age]) => `${label} ${age}`);
  return parts.length ? parts.join(' · ') : null;
}
