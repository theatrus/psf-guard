import type { DirectorRigStatusView } from '../../api/directorTypes';

/** The phases that count as exposing, as the plugin names them. */
export const EXPOSING_PHASES = ['exposing', 'imaging', 'capturing'] as const;

/** The reported phase (or state), lower-cased, or '' when none. */
export function phaseOf(view: DirectorRigStatusView): string {
  const p = view.status?.payload ?? {};
  const phase = typeof p.phase === 'string' ? p.phase : typeof p.state === 'string' ? p.state : '';
  return phase.trim().toLowerCase();
}

/** A fresh report of one of the exposing phases. */
export function statusIsStale(view: DirectorRigStatusView, nowMs: number): boolean {
  const ttl = view.status?.payload.fresh_for_ms;
  // Explicit client leases also expire while a cached query cannot reach the server.
  return view.status_stale || (typeof ttl === 'number' && Number.isFinite(ttl) && !!view.status
    && nowMs - view.status.received_at_ms > Math.min(600_000, Math.max(15_000, ttl)));
}

export function isExposing(view: DirectorRigStatusView, nowMs = Date.now()): boolean {
  return !statusIsStale(view, nowMs) && (EXPOSING_PHASES as readonly string[]).includes(phaseOf(view));
}

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
  const stale = statusIsStale(view, nowMs);
  const phase = text(p.phase) ?? text(p.state) ?? 'reported';
  const parts = [phase];
  const target = text(p.target_name) ?? text(p.target) ?? text(p.target_id);
  if (target) parts.push(target);
  const operation = text(p.operation) ?? text(p.current_operation);
  if (operation) {
    const started = typeof p.operation_started_ms === 'number' ? p.operation_started_ms : null;
    // Monotonic plugin time survives clock skew. Stale reports freeze at the snapshot.
    const elapsed = typeof p.operation_elapsed_ms === 'number' ? p.operation_elapsed_ms
      : started ? Math.max(0, status.reported_at_ms - started) : null;
    const age = stale ? 0 : Math.max(0, nowMs - status.received_at_ms);
    parts.push(elapsed !== null ? `${operation} for ${formatAge(elapsed + age).replace(' ago', '')}` : operation);
  }
  const wait = text(p.wait_reason);
  if (wait) parts.push(`waiting: ${wait}`);
  const safety = text(p.safety);
  if (safety) parts.push(`safety ${safety}`);
  if (typeof p.queue_depth === 'number') parts.push(`${p.queue_depth} queued`);
  else if (text(p.queue_state)) parts.push(`check-in ${text(p.queue_state)}`);
  const line = parts.join(', ');
  return stale ? `${line} (stale, reported ${formatAge(nowMs - status.received_at_ms)})` : line;
}

export function errorsOf(view: DirectorRigStatusView): string[] {
  const p = view.status?.payload ?? {};
  const list = Array.isArray(p.errors) ? p.errors : p.error ? [p.error] : [];
  return list.map(text).filter((e): e is string => !!e).slice(0, 3);
}
