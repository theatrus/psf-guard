import type { DirectorRigStatusView } from '../../api/directorTypes';

/** "40 s ago", "12 min ago", "3 h ago", "2 d ago". */
export function formatAge(ageMs: number): string {
  const s = Math.max(0, Math.round(ageMs / 1000));
  if (s < 60) return `${s} s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `${h} h ago`;
  return `${Math.round(h / 24)} d ago`;
}

const text = (value: unknown): string | null => typeof value === 'string' && value.trim() ? value : typeof value === 'number' && Number.isFinite(value) ? String(value) : null;

/** What the rig says it is doing, from the plugin's coalesced report. The
 *  payload is the plugin's; only the fields the design names are read, and
 *  anything else is left alone. */
export function describeNow(view: DirectorRigStatusView, nowMs: number): string {
  const { status } = view;
  if (!status) return view.connectivity.state === 'never' ? 'No report yet' : 'No status report yet';
  const p = status.payload;
  const phase = text(p.phase) ?? text(p.state) ?? 'reported';
  const parts = [phase];
  const target = text(p.target_name) ?? text(p.target) ?? text(p.target_id);
  if (target) parts.push(target);
  const operation = text(p.operation) ?? text(p.current_operation);
  if (operation) {
    const started = typeof p.operation_started_ms === 'number' ? p.operation_started_ms : null;
    parts.push(started ? `${operation} for ${formatAge(nowMs - started).replace(' ago', '')}` : operation);
  }
  const wait = text(p.wait_reason);
  if (wait) parts.push(`waiting: ${wait}`);
  const safety = text(p.safety);
  if (safety) parts.push(`safety ${safety}`);
  if (typeof p.queue_depth === 'number') parts.push(`${p.queue_depth} queued`);
  const line = parts.join(', ');
  return view.status_stale ? `${line} (stale, reported ${formatAge(nowMs - status.received_at_ms)})` : line;
}

export function errorsOf(view: DirectorRigStatusView): string[] {
  const p = view.status?.payload ?? {};
  const list = Array.isArray(p.errors) ? p.errors : p.error ? [p.error] : [];
  return list.map(text).filter((e): e is string => !!e).slice(0, 3);
}
