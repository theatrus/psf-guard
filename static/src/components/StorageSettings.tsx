import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  CacheVolumeReport,
  StorageFolder,
  StorageFolderKind,
  StorageFoldersUpdate,
  StorageSettings as StorageSettingsData,
} from '../api/types';
import { isTauriApp, tauriConfig, tauriFileSystem } from '../utils/tauri';

const QUERY_KEY = ['storage-settings'] as const;

// Powers, not shifts: a shift in JavaScript is 32-bit, so 1 << 40 is 256.
const KiB = 2 ** 10;
const MiB = 2 ** 20;
const GiB = 2 ** 30;
const TiB = 2 ** 40;

function bytes(count: number): string {
  if (count >= TiB) return `${(count / TiB).toFixed(1)} TiB`;
  if (count >= GiB) return `${(count / GiB).toFixed(1)} GiB`;
  if (count >= MiB) return `${Math.round(count / MiB)} MiB`;
  return `${Math.round(count / KiB)} KiB`;
}

function VolumeUse({ volume }: { volume: CacheVolumeReport }) {
  const used = Math.min(100, volume.used_percent);
  return (
    <div className="storage-volume">
      <div className="storage-volume-head">
        <code title={volume.path}>{volume.path}</code>
        <span className={volume.over_limit ? 'is-over' : 'muted'}>
          {volume.used_percent.toFixed(1)}% used of {bytes(volume.total_bytes)}
        </span>
      </div>
      <div
        className="storage-volume-bar"
        role="meter"
        aria-label={`Disk use of ${volume.path}`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(used)}
      >
        <span className={volume.over_limit ? 'is-over' : ''} style={{ width: `${used}%` }} />
        {volume.max_percent < 100 && (
          <i style={{ left: `${volume.max_percent}%` }} title={`Limit ${volume.max_percent}%`} />
        )}
      </div>
      <p className="muted storage-volume-breakdown">
        Stacks {bytes(volume.stack_bytes)} · calibration masters {bytes(volume.calibration_bytes)} ·
        image previews {bytes(volume.preview_bytes)}
        {volume.other_bytes > 0 && <> · other {bytes(volume.other_bytes)}</>}
        {volume.databases.length > 0 && <> · for {volume.databases.join(', ')}</>}
      </p>
      {volume.culled_files > 0 && (
        <p className="muted storage-volume-note">
          The last check culled {volume.culled_files} previews and checkpoints, {bytes(volume.freed_bytes)}.
        </p>
      )}
      {volume.over_limit && (
        <p className="storage-volume-note is-error" role="alert">
          Over the limit with no previews left to cull. Stacks are never culled, so free space on
          this volume or move the cache to a larger one; preview pre-generation waits meanwhile.
        </p>
      )}
    </div>
  );
}

const FOLDER_LABELS: Record<StorageFolderKind, { name: string; hint: string }> = {
  cache: { name: 'Cache', hint: 'Image previews, star lists and plate solves.' },
  stacks: { name: 'Stacks', hint: 'Stacks, their processing, and WBPP runs. Empty uses the cache.' },
  calibration: {
    name: 'Calibration masters',
    hint: 'Masters built from your calibration frames. Empty uses the cache.',
  },
};

const FOLDER_FIELDS: Record<StorageFolderKind, keyof StorageFoldersUpdate> = {
  cache: 'cache_dir',
  stacks: 'stack_dir',
  calibration: 'calibration_dir',
};

function chosenFolders(folders: StorageFolder[]): Record<StorageFolderKind, string> {
  const chosen = { cache: '', stacks: '', calibration: '' };
  for (const folder of folders) chosen[folder.kind] = folder.chosen ?? '';
  return chosen;
}

/**
 * Where the cache, stacks and masters go. A change takes effect when the
 * server next starts, which moves the files across first.
 */
function StorageFolders({ current, canManage }: { current: StorageSettingsData; canManage: boolean }) {
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState(() => chosenFolders(current.folders));
  const [restarting, setRestarting] = useState(false);
  // Follow a save made elsewhere unless this form has its own edits.
  const savedKey = JSON.stringify(chosenFolders(current.folders));
  const lastSaved = useRef(savedKey);
  useEffect(() => {
    if (lastSaved.current === savedKey) return;
    const previous = lastSaved.current;
    lastSaved.current = savedKey;
    setDraft((draftNow) =>
      JSON.stringify(draftNow) === previous ? (JSON.parse(savedKey) as typeof draftNow) : draftNow,
    );
  }, [savedKey]);
  const save = useMutation({
    mutationFn: apiClient.updateStorageFolders,
    onSuccess: (updated) => {
      queryClient.setQueryData(QUERY_KEY, updated);
      const saved = chosenFolders(updated.folders);
      lastSaved.current = JSON.stringify(saved);
      setDraft(saved);
    },
  });
  const saved = chosenFolders(current.folders);
  const dirty = current.folders.some((folder) => draft[folder.kind].trim() !== saved[folder.kind]);
  const pending = current.folders.some((folder) => folder.path !== folder.next_path);
  const editable = canManage && current.can_choose_folders;

  // The whole app, not just its server: files move only at a fresh start,
  // when nothing from the old server is still writing.
  const restart = async () => {
    setRestarting(true);
    if (!(await tauriConfig.restartApplication())) setRestarting(false);
  };

  const submit = () => {
    const update: StorageFoldersUpdate = {};
    for (const folder of current.folders) {
      if (folder.source !== 'server_config') update[FOLDER_FIELDS[folder.kind]] = draft[folder.kind].trim();
    }
    save.mutate(update);
  };

  return (
    <div className="storage-folders">
      <h4>Folders</h4>
      {current.folders.map((folder) => {
        const label = FOLDER_LABELS[folder.kind];
        const fixed = folder.source === 'server_config';
        const inputId = `storage-folder-${folder.kind}`;
        return (
          <div className="storage-folder" key={folder.kind}>
            <label htmlFor={inputId} className="stack-method-label">
              {label.name}
            </label>
            <div className="storage-folder-input">
              <input
                id={inputId}
                type="text"
                value={fixed ? folder.next_path : draft[folder.kind]}
                placeholder={folder.kind === 'cache' ? folder.next_path : 'Same as the cache'}
                disabled={!editable || fixed || save.isPending}
                aria-describedby={`${inputId}-hint`}
                onChange={(event) => setDraft({ ...draft, [folder.kind]: event.target.value })}
                spellCheck={false}
              />
              {isTauriApp() && editable && !fixed && (
                <button
                  type="button"
                  className="btn btn-secondary"
                  disabled={save.isPending}
                  onClick={async () => {
                    const picked = await tauriFileSystem.pickImageDirectory();
                    if (picked) setDraft((previous) => ({ ...previous, [folder.kind]: picked }));
                  }}
                >
                  Browse…
                </button>
              )}
            </div>
            <small className="muted" id={`${inputId}-hint`}>
              {fixed ? 'Set by the server config file.' : label.hint}
              {folder.path !== folder.next_path && (
                <>
                  {' '}
                  In use until the restart: <code>{folder.path}</code>
                </>
              )}
            </small>
          </div>
        );
      })}
      {current.folder_notes.map((note) => (
        <p key={note} className="muted storage-volume-note">
          {note}
        </p>
      ))}
      {!current.can_choose_folders && (
        <p className="muted">This server keeps no settings file, so only its config file sets the folders.</p>
      )}
      {editable && (
        <div className="storage-folder-actions">
          <button type="button" className="btn btn-primary" disabled={!dirty || save.isPending} onClick={submit}>
            {save.isPending ? 'Saving…' : 'Save folders'}
          </button>
          {pending && (
            <span className="muted">
              The new folders take effect when PSF Guard next starts, which moves the files across
              first. A large move delays that start.
            </span>
          )}
          {pending && isTauriApp() && (
            <button type="button" className="btn btn-secondary" onClick={restart} disabled={restarting}>
              {restarting ? 'Restarting…' : 'Restart now'}
            </button>
          )}
        </div>
      )}
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
    </div>
  );
}

/**
 * How full the cache's volume may get. Past the limit, image previews go
 * first, least recently used first, so stacks keep their room.
 */
export default function StorageSettings({ canManage }: { canManage: boolean }) {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: QUERY_KEY,
    queryFn: apiClient.getStorageSettings,
    refetchInterval: 60_000,
  });
  const [draft, setDraft] = useState<number | null>(null);
  useEffect(() => {
    if (settings.data) setDraft(settings.data.max_volume_percent);
  }, [settings.data]);
  const save = useMutation({
    mutationFn: apiClient.updateStorageSettings,
    onSuccess: (updated) => queryClient.setQueryData(QUERY_KEY, updated),
  });

  if (settings.isLoading || draft === null) return null;
  if (settings.isError || !settings.data) {
    return (
      <div className="stack-method-settings">
        <h3>Disk use</h3>
        <p className="muted" role="alert">Could not load the disk limit.</p>
      </div>
    );
  }
  const current = settings.data;
  const commit = () => {
    if (!save.isPending && draft !== current.max_volume_percent) save.mutate(draft);
  };

  return (
    <div className="stack-method-settings storage-settings">
      <h3>Disk use</h3>
      <p className="muted">
        The cache holds stacks, calibration masters and image previews. When its volume fills past
        the limit, image previews go first, least recently viewed first, until it is two points
        under; stacks, color previews and masters are never culled. Previews come back the next time
        someone opens the image.
      </p>
      {current.volumes.length === 0 ? (
        <p className="muted">The first check runs a minute after the server starts.</p>
      ) : (
        current.volumes.map((volume) => <VolumeUse key={volume.path} volume={volume} />)
      )}
      {/* Not disabled while saving: that would take the slider out of a keyboard user's hands. */}
      <fieldset className="calibration-settings-group" disabled={!canManage}>
        <div className="review-preference worker-share">
          <span>
            <span className="stack-method-label">
              Most of the volume to use
              <em className="worker-share-value">{draft >= 100 ? 'no limit' : `${draft}%`}</em>
            </span>
            <small>
              Of the whole volume, other files included. Default {current.default_max_volume_percent}%;
              100% turns culling off.
              {!canManage && ' Changing it needs database management on this server.'}
            </small>
          </span>
          <input
            type="range"
            min={current.min_max_volume_percent}
            max={100}
            step={1}
            value={draft}
            aria-label="Most of the cache volume to use, in percent"
            onChange={(event) => setDraft(Number(event.target.value))}
            // Saved when let go, on Enter, or on leaving it: each save also
            // checks the volume, so not on every arrow press.
            onPointerUp={commit}
            onKeyDown={(event) => {
              if (event.key === 'Enter') commit();
            }}
            onBlur={commit}
          />
        </div>
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
      <StorageFolders current={current} canManage={canManage} />
    </div>
  );
}
