import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { AstroBinFilterEntry, AstroBinSeenFilter } from '../api/types';
import Dialog from './Dialog';
import { ASTROBIN_FILTERS_URL, astrobinFilterUrl, parseAstrobinFilterId } from '../utils/astrobin';
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

type Field = keyof Omit<Draft, 'key'>;

/** What stops a row from saving, and which field it is about. */
function problem(draft: Draft): { field: Field; message: string } | null {
  if (!draft.filter_name.trim()) return { field: 'filter_name', message: 'needs a filter name' };
  if (parseAstrobinFilterId(draft.astrobin_id) === null)
    return { field: 'astrobin_id', message: 'needs an AstroBin id or page address' };
  for (const field of ['from_night', 'to_night'] as const) {
    if (draft[field].trim() && !NIGHT.test(draft[field].trim()))
      return { field, message: 'nights are YYYY-MM-DD' };
  }
  if (draft.from_night.trim() && draft.to_night.trim() && draft.from_night.trim() > draft.to_night.trim())
    return { field: 'to_night', message: 'ends before it starts' };
  return null;
}

function toEntry(draft: Draft): AstroBinFilterEntry {
  const optional = (value: string) => (value.trim() ? value.trim() : undefined);
  return {
    filter_name: draft.filter_name.trim(),
    astrobin_id: parseAstrobinFilterId(draft.astrobin_id) ?? 0,
    label: optional(draft.label),
    from_night: optional(draft.from_night),
    to_night: optional(draft.to_night),
  };
}

/** Names match as the export matches them: trimmed, ignoring case. */
const sameName = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

function seenText(seen: AstroBinSeenFilter): string {
  const frames = `${seen.frames} light${seen.frames === 1 ? '' : 's'}`;
  return seen.first_night === seen.last_night
    ? `${frames} on ${seen.first_night}`
    : `${frames}, ${seen.first_night} to ${seen.last_night}`;
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
  const defaults = useQuery({
    queryKey: ['astrobin-settings'],
    queryFn: apiClient.getAstroBinSettings,
    enabled: canManage,
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
  const update = (key: number, field: Field, value: string) =>
    setDrafts((prev) => (prev ?? []).map((row) => (row.key === key ? { ...row, [field]: value } : row)));

  const seen = map.data?.seen ?? [];
  const seenFor = (name: string) => (name.trim() ? seen.find((s) => sameName(s.filter_name, name)) : undefined);
  const notMapped = seen.filter((s) => !rows.some((row) => sameName(row.filter_name, s.filter_name)));
  // A name the server-wide defaults already know starts with that id.
  const fromCatalog = (names: AstroBinSeenFilter[]) => {
    const ids = defaults.data?.filter_ids ?? {};
    const drafted = names.map((s) => {
      const known = Object.entries(ids).find(([name]) => sameName(name, s.filter_name));
      return { ...emptyDraft(), filter_name: s.filter_name, astrobin_id: known ? String(known[1]) : '' };
    });
    setDrafts((prev) => [...(prev ?? []), ...drafted]);
  };

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
        Link each filter name in this catalog to its filter in AstroBin&apos;s{' '}
        <a href={ASTROBIN_FILTERS_URL} target="_blank" rel="noreferrer">
          equipment database
        </a>
        : paste the filter&apos;s page address or its number. Set a first or last night when the rig
        changed filters; where ranges overlap, the entry that starts latest wins. A name with no
        entry uses the server-wide defaults under Setups.
      </p>
      {map.isError && <p className="astrobin-error">{(map.error as Error).message}</p>}
      {canManage && drafts !== null && notMapped.length > 0 && (
        <div className="astrobin-filter-map-seen" role="group" aria-label="Filters not in the map">
          <span className="astrobin-filter-map-seen-title">Not in the map:</span>
          {notMapped.map((s) => (
            <button
              key={s.filter_name}
              type="button"
              className="astrobin-filter-map-chip"
              title={seenText(s)}
              aria-label={`Add ${s.filter_name}, ${seenText(s)}`}
              onClick={() => fromCatalog([s])}
            >
              + {s.filter_name}
            </button>
          ))}
          {notMapped.length > 1 && (
            <button type="button" className="header-button" onClick={() => fromCatalog(notMapped)}>
              Add all
            </button>
          )}
        </div>
      )}
      {drafts !== null && rows.length === 0 && <p className="astrobin-muted">No entries yet.</p>}
      {drafts !== null && rows.length > 0 && (
        <ul className="astrobin-filter-entries">
          {rows.map((row, index) => {
            const rowProblem = problems[index];
            const invalid = (field: Field) => rowProblem?.field === field || undefined;
            const id = parseAstrobinFilterId(row.astrobin_id);
            const seenRow = seenFor(row.filter_name);
            return (
              <li key={row.key} className="astrobin-filter-entry">
                <label className="astrobin-filter-field astrobin-filter-field-name">
                  <span>Filter</span>
                  <input
                    type="text"
                    aria-label={`Filter name, row ${index + 1}`}
                    aria-invalid={invalid('filter_name')}
                    value={row.filter_name}
                    placeholder="G"
                    disabled={!canManage}
                    onChange={(e) => update(row.key, 'filter_name', e.target.value)}
                  />
                </label>
                <div className="astrobin-filter-field astrobin-filter-field-id">
                  <label>
                    <span>AstroBin id</span>
                    <input
                      type="text"
                      aria-label={`AstroBin id, row ${index + 1}`}
                      aria-invalid={invalid('astrobin_id')}
                      value={row.astrobin_id}
                      placeholder="4051 or page address"
                      disabled={!canManage}
                      onChange={(e) => update(row.key, 'astrobin_id', e.target.value)}
                      // A pasted page address keeps only its id once read.
                      onBlur={() => {
                        if (id !== null && row.astrobin_id.trim() !== String(id))
                          update(row.key, 'astrobin_id', String(id));
                      }}
                    />
                  </label>
                  {id !== null && (
                    <a
                      href={astrobinFilterUrl(id)}
                      target="_blank"
                      rel="noreferrer"
                      aria-label={`Open filter ${id} on AstroBin`}
                      title="Open this filter's page on AstroBin"
                    >
                      ↗
                    </a>
                  )}
                </div>
                <label className="astrobin-filter-field astrobin-filter-field-label">
                  <span>What it is</span>
                  <input
                    type="text"
                    aria-label={`Label, row ${index + 1}`}
                    value={row.label}
                    placeholder="Antlia V-Pro G 36mm"
                    disabled={!canManage}
                    onChange={(e) => update(row.key, 'label', e.target.value)}
                  />
                </label>
                <label className="astrobin-filter-field astrobin-filter-field-from">
                  <span>First night</span>
                  <input
                    type="date"
                    aria-label={`First night, row ${index + 1}`}
                    aria-invalid={invalid('from_night')}
                    value={row.from_night}
                    disabled={!canManage}
                    onChange={(e) => update(row.key, 'from_night', e.target.value)}
                  />
                </label>
                <label className="astrobin-filter-field astrobin-filter-field-to">
                  <span>Last night</span>
                  <input
                    type="date"
                    aria-label={`Last night, row ${index + 1}`}
                    aria-invalid={invalid('to_night')}
                    value={row.to_night}
                    disabled={!canManage}
                    onChange={(e) => update(row.key, 'to_night', e.target.value)}
                  />
                </label>
                {canManage && (
                  <button
                    type="button"
                    className="header-button astrobin-filter-remove"
                    aria-label={`Remove row ${index + 1}`}
                    title="Remove this entry"
                    onClick={() => setDrafts((prev) => (prev ?? []).filter((r) => r.key !== row.key))}
                  >
                    ×
                  </button>
                )}
                {(rowProblem || seenRow) && (
                  <div className="astrobin-filter-entry-note">
                    {rowProblem && <small className="astrobin-problem">{rowProblem.message}</small>}
                    {seenRow && <small className="astrobin-muted">{seenText(seenRow)}</small>}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
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
