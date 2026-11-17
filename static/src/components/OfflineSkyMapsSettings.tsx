import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { InstalledSkyMap, SkyMapSet, SkyMapsView } from '../api/types';
import { gigabytes, skyMapsKey } from './skyMapsShared';

/** Framing's list of layers, which an install, a removal or a move changes. */
const surveysKey = ['directorSurveys'] as const;

/**
 * Settings › Storage & backups › Offline sky maps: the sets N.I.N.A.
 * publishes for Planning's framing view, which of them are installed, and
 * the folder they are kept in. A set downloads only when someone clicks
 * Install, and the server checks it against its recorded size and SHA-256.
 */
export default function OfflineSkyMapsSettings({ canManage }: {
  /** Whether this person may delete files or set a path on the server. */
  canManage: boolean;
}) {
  const queryClient = useQueryClient();
  const maps = useQuery({
    queryKey: skyMapsKey,
    queryFn: apiClient.getSkyMaps,
    refetchOnMount: 'always',
    refetchOnWindowFocus: false,
    // Follow an install under way; otherwise the page asks once.
    refetchInterval: (query) => query.state.data?.install.running ? 1000 : false,
  });
  const data = maps.data;
  const [folder, setFolder] = useState<string | null>(null);

  const show = (view: SkyMapsView) => {
    queryClient.setQueryData(skyMapsKey, view);
    void queryClient.invalidateQueries({ queryKey: surveysKey });
  };
  const refresh = () => queryClient.invalidateQueries({ queryKey: skyMapsKey });
  const install = useMutation({
    mutationFn: (set: string) => apiClient.installSkyMap(set),
    onSuccess: (progress) => {
      queryClient.setQueryData<SkyMapsView>(skyMapsKey, (current) =>
        current ? { ...current, install: progress } : current);
      return refresh();
    },
  });
  const stop = useMutation({ mutationFn: apiClient.stopSkyMapInstall, onSuccess: refresh });
  const remove = useMutation({ mutationFn: (name: string) => apiClient.removeSkyMap(name), onSuccess: show });
  const move = useMutation({ mutationFn: (name: string) => apiClient.moveSkyMap(name), onSuccess: show });
  const saveFolder = useMutation({
    mutationFn: (value: string) => apiClient.updateSkyMapsSettings(value),
    onSuccess: (view) => { show(view); setFolder(null); },
  });

  const progress = data?.install;
  const running = !!progress?.running;
  // Framing keeps its list of layers until told: tell it when an install
  // that ended here put a map in place.
  const installedAt = !running && !progress?.error ? progress?.finished_at : undefined;
  useEffect(() => {
    if (installedAt !== undefined) void queryClient.invalidateQueries({ queryKey: surveysKey });
  }, [installedAt, queryClient]);
  const busy = running || install.isPending || remove.isPending || move.isPending;
  const savedFolder = data?.chosen ? data.folder ?? '' : '';
  const shownFolder = folder ?? savedFolder;
  const installedOf = (set: SkyMapSet) => data?.installed.find((map) => map.set === set.key);
  // Maps a person copied in, or installed from a URL: not one of the sets.
  const others = data?.installed.filter((map) => !map.set) ?? [];
  const failure = install.error ?? stop.error ?? remove.error ?? move.error ?? saveFolder.error;

  const removeMap = (map: InstalledSkyMap) => {
    if (window.confirm(`Remove ${map.name}? This deletes its ${gigabytes(map.bytes)} of files from the server. You can install it again.`)) {
      remove.mutate(map.folder);
    }
  };
  const mapActions = (map: InstalledSkyMap) => canManage && (
    <span className="catalog-actions">
      {map.in_cache && (
        <button type="button" className="browse-button" disabled={busy}
          aria-label={`Move ${map.name} to the offline sky maps folder`}
          onClick={() => move.mutate(map.folder)}>
          {move.isPending && move.variables === map.folder ? 'Moving…' : 'Move'}
        </button>
      )}
      <button type="button" className="browse-button" disabled={busy}
        aria-label={`Remove ${map.name}`} onClick={() => removeMap(map)}>
        {remove.isPending && remove.variables === map.folder ? 'Removing…' : 'Remove'}
      </button>
    </span>
  );
  const inCacheNote = (map: InstalledSkyMap) => map.in_cache && (
    <small>
      In the cache folder, where earlier versions installed maps.
      {canManage ? ' Move it so that clearing the cache cannot remove it.' : ''}
    </small>
  );

  return (
    <section className="settings-section catalog-settings" aria-labelledby="offline-sky-maps-title">
      <div className="catalog-heading">
        <div>
          <h3 id="offline-sky-maps-title">Offline sky maps</h3>
          <p>
            Whole-sky pictures for Planning’s framing view, so it draws the sky with no connection
            to the survey service. The sets are N.I.N.A.’s, downloaded from nighttime-imaging.eu
            only when you click Install, and checked before they are installed.
          </p>
        </div>
      </div>

      {maps.isError && !data && (
        <div className="catalog-error">Could not read the offline sky maps: {maps.error.message}</div>
      )}
      {maps.isPending && <p className="catalog-footnote" role="status">Reading the offline sky maps…</p>}

      {data?.sets.map((set) => {
        const map = installedOf(set);
        const installing = running && progress?.set === set.key;
        const short = data.free_bytes !== undefined && data.free_bytes < set.room_needed;
        return (
          <div className="catalog-optional" data-set={set.key} key={set.key}>
            <div>
              <strong>{set.name}</strong>
              <small>
                {set.covers} · {gigabytes(set.bytes)} to download
                {map ? ` · ${gigabytes(map.bytes)} on disk, ${map.tiles} tiles` : ''}
              </small>
              {set.licence && <small>{set.licence}</small>}
              {map && inCacheNote(map)}
              {!map && short && (
                <small className="catalog-optional-error">
                  Needs {gigabytes(set.room_needed)} free while it installs; the folder’s volume has {gigabytes(data.free_bytes!)}.
                </small>
              )}
            </div>
            <span className="catalog-optional-state" data-installed={map ? 'true' : 'false'}>
              {map ? 'Installed' : 'Not installed'}
            </span>
            <span className="catalog-actions">
              <button type="button" className="browse-button" disabled={busy}
                aria-label={map ? `Install ${set.name} again` : `Install ${set.name}`}
                onClick={() => install.mutate(set.key)}>
                {installing ? 'Installing…' : map ? 'Install again' : 'Install'}
              </button>
              {map && mapActions(map)}
            </span>
          </div>
        );
      })}

      {others.map((map) => (
        <div className="catalog-optional" data-map={map.folder} key={map.folder}>
          <div>
            <strong>{map.name}</strong>
            <small>{map.folder} · {gigabytes(map.bytes)} on disk, {map.tiles} tiles</small>
            <small>{map.attribution}</small>
            {inCacheNote(map)}
          </div>
          <span className="catalog-optional-state" data-installed="true">Installed</span>
          {mapActions(map) || <span />}
        </div>
      ))}

      {failure && <div className="catalog-error" role="alert">{failure.message}</div>}

      {progress && (running || progress.finished_at !== undefined) && (
        <div className={`catalog-progress catalog-progress-${running ? 'downloading' : progress.error ? 'error' : 'complete'}`}
          role="status" aria-live="polite">
          <strong>{progress.message}</strong>
          {running && progress.bytes_total !== undefined && progress.bytes_completed !== undefined && (
            <>
              <progress value={progress.bytes_completed} max={progress.bytes_total} />
              <small>{gigabytes(progress.bytes_completed)} of {gigabytes(progress.bytes_total)}</small>
            </>
          )}
          {running && (
            <button type="button" className="browse-button" disabled={stop.isPending}
              onClick={() => stop.mutate()}>
              {stop.isPending ? 'Stopping…' : 'Stop install'}
            </button>
          )}
        </div>
      )}

      {data && (
        <p className="catalog-footnote">
          {data.folder ? <>Maps are kept in <code>{data.folder}</code>. </> : null}
          {data.free_bytes !== undefined ? `${gigabytes(data.free_bytes)} free there. ` : ''}
          Survey pictures fetched online take {gigabytes(data.cutouts_bytes)} in the cache folder.
        </p>
      )}

      {data && canManage && (
        <form className="catalog-install-controls" onSubmit={(event) => {
          event.preventDefault();
          saveFolder.mutate(shownFolder);
        }}>
          <label htmlFor="offline-sky-maps-folder">Offline sky maps folder</label>
          <input id="offline-sky-maps-folder" type="text" value={shownFolder}
            placeholder={data.default_folder ?? ''}
            onChange={(event) => setFolder(event.target.value)} />
          <small>
            Leave empty to use the default folder. Maps already installed stay where they are:
            after a change, copy their folders into the new folder or install them again.
          </small>
          <div className="catalog-actions">
            <button type="submit" className="browse-button"
              disabled={saveFolder.isPending || busy || shownFolder.trim() === savedFolder}>
              {saveFolder.isPending ? 'Saving…' : 'Save folder'}
            </button>
          </div>
        </form>
      )}
    </section>
  );
}
