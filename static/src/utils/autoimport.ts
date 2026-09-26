import type { AutoImportSettings, AutoImportStatus } from '../api/types';
import { describeImportProgress } from '../hooks/useImportJob';

/** Interval choices, in minutes. 0 means only on open. */
export const AUTOIMPORT_INTERVALS: Array<{ minutes: number; label: string }> = [
  { minutes: 0, label: 'Only on open' },
  { minutes: 15, label: 'Every 15 minutes' },
  { minutes: 30, label: 'Every 30 minutes' },
  { minutes: 60, label: 'Every hour' },
  { minutes: 180, label: 'Every 3 hours' },
  { minutes: 360, label: 'Every 6 hours' },
  { minutes: 1440, label: 'Every day' },
];

export function scheduleWords(settings: AutoImportSettings): string {
  const parts: string[] = [];
  if (settings.on_open) parts.push('on open');
  if (settings.interval_minutes > 0) {
    const minutes = settings.interval_minutes;
    parts.push(
      minutes % 1440 === 0
        ? `every ${minutes / 1440 === 1 ? 'day' : `${minutes / 1440} days`}`
        : minutes % 60 === 0
          ? `every ${minutes / 60 === 1 ? 'hour' : `${minutes / 60} hours`}`
          : `every ${minutes} minutes`
    );
  }
  const scope =
    settings.scope === 'lights'
      ? 'lights'
      : settings.scope === 'calibration'
        ? 'calibration frames'
        : 'lights and calibration';
  return `${parts.join(' and ') || 'off'}, ${scope}`;
}

/** "5 minutes ago", "in 25 minutes", from unix seconds. */
export function relativeTime(seconds: number, now = Date.now()): string {
  const delta = Math.round(seconds - now / 1000);
  const abs = Math.abs(delta);
  const unit =
    abs < 60
      ? 'moments'
      : abs < 3600
        ? `${Math.round(abs / 60)} min`
        : abs < 86_400
          ? `${Math.round(abs / 3600)} h`
          : `${Math.round(abs / 86_400)} d`;
  if (abs < 60) return delta < 0 ? 'moments ago' : 'in moments';
  return delta < 0 ? `${unit} ago` : `in ${unit}`;
}

/** One line for the status: what the last automatic run did, or when the next is due. */
export function describeAutoImport(
  settings: AutoImportSettings,
  status: AutoImportStatus | undefined,
  now = Date.now()
): string {
  const head = `Auto import ${scheduleWords(settings)}`;
  if (!status) return head;
  const progress = status.progress ?? undefined;
  if (progress?.running) {
    return `${head} · ${describeImportProgress(progress)}`;
  }
  const details: string[] = [];
  if (progress && status.last_started_at) {
    const sentence = describeImportProgress(progress);
    details.push(`last run ${relativeTime(status.last_started_at, now)}: ${sentence}`);
  } else if (status.last_started_at) {
    details.push(`last run ${relativeTime(status.last_started_at, now)}`);
  }
  if (status.next_run_at) {
    details.push(`next ${relativeTime(status.next_run_at, now)}`);
  }
  return details.length ? `${head} · ${details.join(' · ')}` : head;
}
