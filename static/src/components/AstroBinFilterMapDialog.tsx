import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { AstroBinFilterEntry } from '../api/types';
import Dialog from './Dialog';
import './AstroBinFilterMapDialog.css';

interface Props {
  dbId: string;
  dbName: string;
  canManage: boolean;
  onClose: () => void;
}

/** A row as edited: every field is text until it is saved. */
interface Draft {
  key: number;
  filter_name: string;
  astrobin_id: string;
  label: string;
  from_night: string;
  to_night: string;
}

let nextKey = 1;

function toDraft(entry: AstroBinFilterEntry): Draft {
  return {
    key: nextKey++,
    filter_name: entry.filter_name,
    astrobin_id: String(entry.astrobin_id),
    label: entry.label ?? '',
    from_night: entry.from_night ?? '',
    to_night: entry.to_night ?? '',
  };
}

function emptyDraft(): Draft {
  return { key: nextKey++, filter_name: '', astrobin_id: '', label: '', from_night: '', to_night: '' };
}

const NIGHT = /^\d{4}-\d{2}-\d{2}$/;

function problem(draft: Draft): string | null {
  if (!draft.filter_name.trim()) return 'needs a filter name';
  if (!/^\d+$/.test(draft.astrobin_id.trim()) || Number(draft.astrobin_id) <= 0)
    return 'needs an AstroBin id';
  for (const night of [draft.from_night, draft.to_night]) {
    if (night.trim() && !NIGHT.test(night.trim())) return 'nights are YYYY-MM-DD';
  }
  if (draft.from_night.trim() && draft.to_night.trim() && draft.from_night.trim() > draft.to_night.trim())
    return 'ends before it starts';
  return null;
}

function toEntry(draft: Draft): AstroBinFilterEntry {
  const optional = (value: string) => (value.trim() ? value.trim() : undefined);
  return {
    filter_name: draft.filter_name.trim(),
    astrobin_id: Number(draft.astrobin_id.trim()),
    label: optional(draft.label),
    from_night: optional(draft.from_night),
    to_night: optional(draft.to_night),
  };
}

/**
 * The catalog's filter map: what each filter name meant on this rig, and
 * over which nights. The AstroBin export reads it ahead of the server-wide
 * defaults, so a rig whose "G" changed in 2026 can say so.
 */
export default function AstroBinFilterMapDialog({ dbId, dbName, canManage, onClose }: Props) {
  const queryClient = useQueryClient();
  const map = useQuery({
    queryKey: ['db', dbId, 'astrobin-filters'],
    queryFn: () => apiClient.getAstroBinFilters(dbId),
  });
  const [drafts, setDrafts] = useState<Draft[] | null>(null);
  useEffect(() => {
    if (map.data && drafts === null) setDrafts(map.data.entries.map(toDraft));
  }, [map.data, drafts]);

  const save = useMutation({
    mutationFn: (entries: AstroBinFilterEntry[]) => apiClient.updateAstroBinFilters(dbId, entries),
    onSuccess: (saved) => {
      queryClient.setQueryData(['db', dbId, 'astrobin-filters'], saved);
      setDrafts(saved.entries.map(toDraft));
      void queryClient.invalidateQueries({ queryKey: ['astrobin-export', dbId] });
    },
  });

  const rows = drafts ?? [];
  const problems = rows.map(problem);
  const canSave = canManage && problems.every((p) => p === null) && !save.isPending;
  const update = (key: number, field: keyof Omit<Draft, 'key'>, value: string) =>
    setDrafts((prev) => (prev ?? []).map((row) => (row.key === key ? { ...row, [field]: value } : row)));

  return (
    <Dialog
      open
      title={`AstroBin filters — ${dbName}`}
      onClose={onClose}
      className="astrobin-filter-map"
      footer={
        <>
          <button type="button" className="header-button" onClick={onClose}>
            Close
          </button>
          {canManage && (
            <button
              type="button"
              className="action-button"
              disabled={!canSave}
              onClick={() => save.mutate(rows.map(toEntry))}
            >
              Save map
            </button>
          )}
        </>
      }
    >
      <p className="astrobin-filter-map-intro">
        AstroBin knows a filter by the number in the address of its page in the{' '}
        <a href="https://app.astrobin.com/equipment/explorer/filter" target="_blank" rel="noreferrer">
          equipment database
        </a>
        . This map says which filter each name in this catalog stands for. Give an entry a first
        or last night when the rig&apos;s filter changed; the entry that starts latest wins on a
        night more than one covers. A name with no entry here falls back to the server-wide
        defaults under Setups.
      </p>
      {map.isError && <p className="astrobin-error">{(map.error as Error).message}</p>}
      {drafts !== null && (
        <div className="astrobin-filter-map-table-wrap">
          <table className="astrobin-filter-map-table">
            <thead>
              <tr>
                <th>Filter name</th>
                <th>AstroBin id</th>
                <th>What it is</th>
                <th>First night</th>
                <th>Last night</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {rows.length === 0 && (
                <tr>
                  <td colSpan={6} className="astrobin-muted">
                    No entries yet.
                  </td>
                </tr>
              )}
              {rows.map((row, index) => (
                <tr key={row.key} className={problems[index] ? 'has-problem' : ''}>
                  <td>
                    <input
                      type="text"
                      aria-label={`Filter name, row ${index + 1}`}
                      value={row.filter_name}
                      placeholder="G"
                      disabled={!canManage}
                      onChange={(e) => update(row.key, 'filter_name', e.target.value)}
                    />
                  </td>
                  <td>
                    <input
                      type="text"
                      inputMode="numeric"
                      aria-label={`AstroBin id, row ${index + 1}`}
                      value={row.astrobin_id}
                      placeholder="4051"
                      disabled={!canManage}
                      onChange={(e) => update(row.key, 'astrobin_id', e.target.value)}
                    />
                  </td>
                  <td>
                    <input
                      type="text"
                      aria-label={`Label, row ${index + 1}`}
                      value={row.label}
                      placeholder="Antlia V-Pro G 36mm"
                      disabled={!canManage}
                      onChange={(e) => update(row.key, 'label', e.target.value)}
                    />
                  </td>
                  <td>
                    <input
                      type="text"
                      aria-label={`First night, row ${index + 1}`}
                      value={row.from_night}
                      placeholder="any"
                      disabled={!canManage}
                      onChange={(e) => update(row.key, 'from_night', e.target.value)}
                    />
                  </td>
                  <td>
                    <input
                      type="text"
                      aria-label={`Last night, row ${index + 1}`}
                      value={row.to_night}
                      placeholder="any"
                      disabled={!canManage}
                      onChange={(e) => update(row.key, 'to_night', e.target.value)}
                    />
                  </td>
                  <td>
                    {canManage && (
                      <button
                        type="button"
                        className="header-button"
                        aria-label={`Remove row ${index + 1}`}
                        onClick={() =>
                          setDrafts((prev) => (prev ?? []).filter((r) => r.key !== row.key))
                        }
                      >
                        Remove
                      </button>
                    )}
                    {problems[index] && <small className="astrobin-problem">{problems[index]}</small>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {canManage && drafts !== null && (
        <button
          type="button"
          className="header-button"
          onClick={() => setDrafts((prev) => [...(prev ?? []), emptyDraft()])}
        >
          + Add entry
        </button>
      )}
      {save.isError && <p className="astrobin-error">{(save.error as Error).message}</p>}
    </Dialog>
  );
}
