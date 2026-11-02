import { useQuery } from '@tanstack/react-query';
import { RadioTower } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { liveSummary } from './liveSummary';
import { retryWhenBusy } from '../director/retry';
import './header.css';
import { useDirectorClock } from '../../hooks/useDirectorClock';

interface RigsTabProps {
  current: boolean;
  onClick: () => void;
}

/**
 * The Rigs view in the header, carrying the fleet's status: how many rigs,
 * how many exposing, red when one that used to talk has gone quiet, and
 * each rig's state in the tooltip. Before the first rig it is a plain view
 * tab; with Director off it stays out of the header.
 */
export default function RigsTab({ current, onClick }: RigsTabProps) {
  const director = useDirectorStatus();
  const enabled = !!director.data?.enabled && director.data.protocol_version === 1 && !!director.data.instance_id;
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, enabled, retry: retryWhenBusy, retryDelay: 1200, refetchInterval: 15_000, refetchOnWindowFocus: true });
  const now = useDirectorClock();
  if (!enabled) return null;
  const rows = statuses.data ?? [];
  const summary = rows.length > 0 ? liveSummary(rows, now) : null;
  return (
    <button
      type="button"
      onClick={onClick}
      className={`header-button view-tab rigs-tab${summary?.alert ? ' is-alert' : ''}`}
      data-view="rigs"
      aria-current={current ? 'page' : undefined}
      aria-label={summary ? `Rigs: ${summary.label}` : 'Rigs'}
      title={summary?.title}
    >
      <RadioTower size={16} aria-hidden="true" />
      {summary
        ? <>
            <span className="rigs-tab-dot" aria-hidden="true" />
            <span className="rigs-tab-status" aria-hidden="true">{summary.label}</span>
          </>
        : <span className="view-tab-name" aria-hidden="true">Rigs</span>}
    </button>
  );
}
