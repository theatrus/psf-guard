import { useLocation, useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { isSkyPath, withoutPlanningParams } from '../../hooks/useUrlState';
import { liveSummary } from './liveSummary';
import { retryWhenBusy } from '../director/retry';
import './header.css';
import { useDirectorClock } from '../../hooks/useDirectorClock';

/** The fleet in the header: how many rigs, how many exposing, red when one
 *  has gone quiet. Opens Live on the Sky: each rig drawn where it points,
 *  the list beside the map and the full table under it. It stays out of
 *  the header when Director is off or no rig exists yet. */
export default function LiveChip() {
  const director = useDirectorStatus();
  const enabled = !!director.data?.enabled && director.data.protocol_version === 1 && !!director.data.instance_id;
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, enabled, retry: retryWhenBusy, retryDelay: 1200, refetchInterval: 15_000, refetchOnWindowFocus: true });
  const location = useLocation();
  const navigate = useNavigate();
  const rows = statuses.data ?? [];
  const now = useDirectorClock();
  if (!enabled || rows.length === 0) return null;
  const summary = liveSummary(rows, now);
  const onLive = isSkyPath(location.pathname) && new URLSearchParams(location.search).get('live') === '1';
  const open = () => {
    if (onLive) return;
    const next = withoutPlanningParams(location.search);
    next.set('live', '1');
    navigate(`/sky?${next}`);
  };
  return <button type="button" className={`header-button utility-button live-chip${summary.alert ? ' is-alert' : ''}`} title={summary.title}
    aria-label={`Live rigs: ${summary.label}`} aria-current={onLive ? 'page' : undefined} onClick={open}>
    <span className="live-chip-dot" aria-hidden="true" />
    <span className="utility-label">{summary.label}</span>
  </button>;
}
