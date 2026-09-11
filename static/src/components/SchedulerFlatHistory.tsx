import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronLeft, ChevronRight, RefreshCw, ShieldOff } from 'lucide-react';
import { apiClient } from '../api/client';
import type { FlatHistoryState, SchedulerFlatHistoryRecord } from '../api/types';
import './SchedulerFlatHistory.css';

const PAGE_SIZE = 100;
const STATES: Record<FlatHistoryState, string> = {
  recorded: 'Recorded', pending: 'Awaiting sync', removed: 'Coverage removed',
  absent: 'Already absent', conflict: 'Changed in NINA', superseded: 'Replaced',
};

function timestamp(value: number | null): string {
  return value === null ? 'Unknown' : new Date(value * 1000).toLocaleString();
}

function settings(row: SchedulerFlatHistoryRecord): string {
  return [
    row.gain !== null ? `Gain ${row.gain}` : null,
    row.offset !== null ? `offset ${row.offset}` : null,
    row.bin !== null ? `bin ${row.bin}` : null,
    row.readout_mode !== null ? `readout ${row.readout_mode}` : null,
    row.roi !== null ? `ROI ${row.roi}` : null,
    row.rotation !== null ? `rotation ${row.rotation}` : null,
  ].filter(Boolean).join(' / ') || 'Settings not recorded';
}

export default function SchedulerFlatHistory({ dbId, canManage }: {
  dbId: string; canManage: boolean;
}) {
  const queryClient = useQueryClient();
  const [state, setState] = useState<FlatHistoryState | 'all'>('recorded');
  const [query, setQuery] = useState('');
  const [search, setSearch] = useState('');
  const [offset, setOffset] = useState(0);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [reason, setReason] = useState('');
  const [message, setMessage] = useState('');
  const history = useQuery({
    queryKey: ['db', dbId, 'flat-history', state, search, offset],
    queryFn: () => apiClient.getSchedulerFlatHistory(dbId, {
      limit: PAGE_SIZE, offset, state: state === 'all' ? undefined : state,
      q: search || undefined,
    }),
  });
  const rows = history.data?.records ?? [];
  const eligible = rows.filter((row) => row.state === 'recorded');
  // A refresh can replace a selected row. Only the current immutable records
  // still eligible for invalidation may be submitted.
  const selectedIds = eligible.filter((row) => selected.has(row.record_id))
    .map((row) => row.record_id);
  const invalidate = useMutation({
    mutationFn: (input: { ids: string[]; reason: string }) =>
      apiClient.invalidateSchedulerFlatHistory(dbId, input.ids, input.reason),
    onSuccess: async ({ invalidated }) => {
      setSelected(new Set());
      setReason('');
      setOffset(0);
      setMessage(`${invalidated} coverage record${invalidated === 1 ? '' : 's'} awaiting sync.`);
      await queryClient.invalidateQueries({ queryKey: ['db', dbId, 'flat-history'] });
    },
  });
  const resetSelection = () => {
    setSelected(new Set());
    setMessage('');
    invalidate.reset();
  };
  const changePage = (next: number) => {
    resetSelection();
    setOffset(next);
  };
  const total = history.data?.total ?? 0;
  useEffect(() => {
    if (history.data && !history.isFetching && !history.error && offset > 0 && offset >= history.data.total) {
      setSelected(new Set());
      setOffset(Math.max(0, Math.floor((history.data.total - 1) / PAGE_SIZE) * PAGE_SIZE));
    }
  }, [history.data, history.isFetching, history.error, offset]);
  return (
    <div className="scheduler-flat-history" role="tabpanel" aria-label="Scheduler flats">
      <form className="flat-history-toolbar" onSubmit={(event) => {
        event.preventDefault();
        resetSelection();
        setSearch(query.trim());
        setOffset(0);
      }}>
        <label className="flat-history-search">Target, source or filter
          <input type="search" value={query} maxLength={200}
            onChange={(event) => setQuery(event.target.value)} />
        </label>
        <button className="browse-button" type="submit" disabled={invalidate.isPending}>Search</button>
        <label>Status
          <select aria-label="Coverage status" value={state} disabled={invalidate.isPending} onChange={(event) => {
            resetSelection(); setOffset(0); setState(event.target.value as typeof state);
          }}>
            <option value="all">All statuses</option>
            {Object.entries(STATES).map(([value, label]) =>
              <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
        <button type="button" className="browse-button flat-history-icon"
          title="Refresh coverage" aria-label="Refresh coverage"
          disabled={history.isFetching || invalidate.isPending}
          onClick={() => { void history.refetch(); }}><RefreshCw size={16} /></button>
      </form>
      {message && <p className="flat-history-message" role="status">{message}</p>}
      {history.error && <p className="calibration-library-error" role="alert">
        Could not load scheduler flat coverage: {history.error.message}
      </p>}
      {invalidate.error && <p className="calibration-library-error" role="alert">
        {invalidate.error.message}
      </p>}
      {canManage && selectedIds.length > 0 && (
        <form className="flat-history-invalidate" onSubmit={(event) => {
          event.preventDefault();
          if (reason.trim() && !history.isFetching && !history.error && !invalidate.isPending) {
            invalidate.mutate({ ids: selectedIds, reason: reason.trim() });
          }
        }}>
          <strong>Invalidate {selectedIds.length} coverage record{selectedIds.length === 1 ? '' : 's'}?</strong>
          <p>This removes the selected coverage from Target Scheduler on the next grade pull or
            applied reconcile. Sync before the next flats action. New flats remain subject to
            scheduler eligibility. Files and calibration masters are unchanged.</p>
          <p>Duplicate coverage can still satisfy the scheduler. Include all suspect records for the run.</p>
          <label>Reason
            <input value={reason} required maxLength={1000} disabled={invalidate.isPending}
              onChange={(event) => setReason(event.target.value)} />
          </label>
          <div>
            <button className="remove-button" type="submit"
              disabled={!reason.trim() || history.isFetching || !!history.error || invalidate.isPending}>
              <ShieldOff size={16} /> {invalidate.isPending ? 'Invalidating...' : 'Invalidate coverage'}
            </button>
            <button className="browse-button" type="button" disabled={invalidate.isPending}
              onClick={resetSelection}>Cancel</button>
          </div>
        </form>
      )}
      <div className="flat-history-table-scroll" aria-busy={history.isFetching}>
        {history.isLoading && <p className="calibration-library-empty">Loading scheduler flat coverage...</p>}
        {!history.isLoading && !history.error && rows.length === 0 && (
          <p className="calibration-library-empty">No scheduler flat coverage matches these filters.</p>
        )}
        {rows.length > 0 && <table className="flat-history-table">
          <thead><tr>
            {canManage && <th><input type="checkbox" aria-label="Select recorded coverage on this page"
              disabled={eligible.length === 0 || invalidate.isPending || history.isFetching}
              checked={eligible.length > 0 && selectedIds.length === eligible.length}
              onChange={(event) => setSelected(new Set(event.target.checked ? eligible.map((r) => r.record_id) : []))} /></th>}
            <th>Target / source</th><th>Light session</th><th>Flat coverage</th><th>Status</th>
          </tr></thead>
          <tbody>{rows.map((row) => <tr key={row.record_id}>
            {canManage && <td>{row.state === 'recorded' && <input type="checkbox"
              aria-label={`Select coverage ${row.source_row_id} for ${row.target_name || 'profile flats'}`}
              checked={selectedIds.includes(row.record_id)} disabled={invalidate.isPending || history.isFetching}
              onChange={(event) => setSelected((current) => {
                const next = new Set(current);
                if (event.target.checked) next.add(row.record_id); else next.delete(row.record_id);
                return next;
              })} />}</td>}
            <td><strong title={row.target_guid ?? undefined}>{row.target_name || 'Profile flats'}</strong>
              <small title={row.origin_id}>{row.source_name} / #{row.source_row_id}</small>
              <small title={row.origin_id}>Source: {row.origin_id.slice(0, 8)}</small>
              <small>Profile: {row.profile_id}</small></td>
            <td>{timestamp(row.light_session_date)}<small>Session {row.light_session_id}</small></td>
            <td><strong>{row.filter_name || 'No filter'}{row.flats_type ? ` / ${row.flats_type}` : ''}</strong>
              <small>{timestamp(row.flats_taken_date)}</small><small>{settings(row)}</small></td>
            <td><span className={`flat-history-state flat-history-state-${row.state}`}>{STATES[row.state]}</span>
              {row.reason && <small>{row.reason}</small>}
              {row.detail && <small>{row.detail}</small>}
              <small>Last seen: {timestamp(row.last_seen)}</small>
              {row.invalidated_at !== null && <small>Invalidated: {timestamp(row.invalidated_at)}</small>}
              {row.acknowledged_at !== null && <small>Synced: {timestamp(row.acknowledged_at)}</small>}
            </td>
          </tr>)}</tbody>
        </table>}
      </div>
      <footer className="flat-history-footer">
        <span>{total === 0 ? '0 records' : rows.length === 0 ? `${total} records` : `${offset + 1}-${offset + rows.length} of ${total} records`}</span>
        <button type="button" className="browse-button flat-history-icon" title="Previous page"
          aria-label="Previous coverage page" disabled={offset === 0 || history.isFetching || invalidate.isPending}
          onClick={() => changePage(Math.max(0, offset - PAGE_SIZE))}><ChevronLeft size={16} /></button>
        <button type="button" className="browse-button flat-history-icon" title="Next page"
          aria-label="Next coverage page" disabled={offset + PAGE_SIZE >= total || history.isFetching || invalidate.isPending}
          onClick={() => changePage(offset + PAGE_SIZE)}><ChevronRight size={16} /></button>
      </footer>
    </div>
  );
}
