import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';

/**
 * The AstroBin export's filter map: which equipment-database id each of the
 * catalog's filter names stands for. Server-wide, persisted in the registry;
 * the export dialog also fills gaps in it as they come up.
 */
export default function AstroBinSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['astrobin-settings'],
    queryFn: apiClient.getAstroBinSettings,
  });
  const [newName, setNewName] = useState('');
  const [newId, setNewId] = useState('');

  const save = useMutation({
    mutationFn: (filterIds: Record<string, number>) =>
      apiClient.updateAstroBinSettings(filterIds),
    onSuccess: (updated) => {
      queryClient.setQueryData(['astrobin-settings'], updated);
      setNewName('');
      setNewId('');
    },
  });

  if (settings.isLoading) return null;
  if (settings.isError) {
    return (
      <div className="astrobin-settings">
        <h3>AstroBin</h3>
        <p className="muted">Could not load AstroBin settings.</p>
      </div>
    );
  }

  const filterIds = settings.data!.filter_ids;
  const names = Object.keys(filterIds).sort();
  const parsedNewId = Number.parseInt(newId.trim(), 10);
  const canAdd = newName.trim().length > 0 && Number.isFinite(parsedNewId) && parsedNewId > 0;

  const add = () => {
    if (!canAdd) return;
    save.mutate({ ...filterIds, [newName.trim()]: parsedNewId });
  };
  const remove = (name: string) => {
    const next = { ...filterIds };
    delete next[name];
    save.mutate(next);
  };

  return (
    <div className="astrobin-settings">
      <h3>AstroBin</h3>
      <p className="review-preferences-note">
        Default AstroBin ids by filter name, for any catalog whose own filter map (under
        Databases) has no entry for that name. AstroBin knows a filter by the number in the
        address of its page in the equipment database. A rig that changed filters keeps the
        history in its catalog&apos;s map; this is the fallback.
      </p>
      {names.length > 0 && (
        <table className="astrobin-settings-table">
          <thead>
            <tr>
              <th>Filter</th>
              <th>AstroBin id</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {names.map((name) => (
              <tr key={name}>
                <td>{name}</td>
                <td>{filterIds[name]}</td>
                <td>
                  <button
                    type="button"
                    className="header-button"
                    disabled={save.isPending}
                    aria-label={`Forget AstroBin id for ${name}`}
                    onClick={() => remove(name)}
                  >
                    Forget
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <div className="astrobin-settings-add">
        <input
          type="text"
          placeholder="Filter name (as in the catalog)"
          aria-label="Filter name"
          value={newName}
          onChange={(event) => setNewName(event.target.value)}
        />
        <input
          type="text"
          inputMode="numeric"
          placeholder="AstroBin filter id"
          aria-label="AstroBin filter id"
          value={newId}
          onChange={(event) => setNewId(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') add();
          }}
        />
        <button
          type="button"
          className="header-button"
          disabled={!canAdd || save.isPending}
          onClick={add}
        >
          Add
        </button>
      </div>
      {save.isError && <p className="error-text">{(save.error as Error).message}</p>}
    </div>
  );
}
