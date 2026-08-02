import type { StackActivityEntry } from '../api/types';
import { useStackActivity } from '../hooks/useStackActivity';
import './CacheRefreshStatus.css';

interface StackActivityStatusProps {
  className?: string;
}

function primaryEntry(active: StackActivityEntry[]): StackActivityEntry | undefined {
  return active.find((entry) => entry.state === 'running') ?? active[0];
}

/**
 * Secondary header indicator for stack builds. Jobs run on the server, so this
 * keeps reporting from any view, including one the build was not started from.
 */
export default function StackActivityStatus({ className = '' }: StackActivityStatusProps) {
  const { active } = useStackActivity();
  const entry = primaryEntry(active);
  if (!entry) return null;

  const percentage = entry.total_units > 0
    ? Math.min((entry.processed_units / entry.total_units) * 100, 100)
    : 0;
  const others = active.length - 1;
  const unit = entry.kind === 'mono' ? 'frames' : 'steps';
  const detail = entry.total_units > 0
    ? `${entry.label} · ${entry.processed_units}/${entry.total_units} ${unit}`
    : `${entry.label} · ${entry.detail}`;

  return (
    <div
      className={`cache-refresh-status visible stack-activity-status ${className}`}
      aria-live="polite"
      title={`${entry.label} · ${entry.detail}`}
    >
      <div className="cache-status-content">
        <div className="progress-indicator">
          <div className="pulsating-bar" />
        </div>
        <div className="cache-status-main">
          <div className="cache-status-label">
            Stacking{others > 0 ? ` +${others}` : ''}
          </div>
          {entry.total_units > 0 && (
            <div className="cache-progress-bar">
              <div className="cache-progress-fill" style={{ width: `${percentage}%` }} />
            </div>
          )}
        </div>
        <div className="cache-status-details">
          <div className="progress-info-row">{detail}</div>
        </div>
      </div>
    </div>
  );
}
