import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  CacheVolumeReport,
  StorageFolder,
  StorageFolderKind,
  StorageFoldersUpdate,
  StorageLimitsUpdate,
  StorageSettings as StorageSettingsData,
} from '../api/types';
import { isTauriApp, tauriConfig } from '../utils/tauri';
import PathField from './PathField';
import UnsavedMark from './UnsavedMark';

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

const KIND_NAMES: Record<StorageFolderKind, string> = {
  cache: 'cache',
  stacks: 'stacks',
  calibration: 'calibration masters',
};

function kindList(kinds: StorageFolderKind[]): string {
  const names = kinds.map((kind) => KIND_NAMES[kind]);
  return names.length < 2 ? names.join('') : `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`;
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
        {volume.kinds.length > 0 && <> · holds the {kindList(volume.kinds)}</>}
      </p>
      {volume.culled_files > 0 && (
        <p className="muted storage-volume-note">
          The last check deleted {volume.culled_files} files, {bytes(volume.freed_bytes)}.
        </p>
      )}
      {volume.over_limit && (
        <p className="storage-volume-note is-error" role="alert">
          {volume.kinds.includes('cache')
            ? 'Disk usage exceeds the limit, and no more files can be deleted automatically. PSF Guard preserves stacks. Free space on this volume or move a folder to a larger volume. Automatic preview generation is paused until space is available.'
            : 'Disk usage exceeds the limit, and no more files can be deleted automatically. PSF Guard preserves stacks, masters used within the last week, and masters with missing source frames. Free space on this volume or move the folder to a larger volume.'}
        </p>
      )}
    </div>
  );
}

const FOLDER_LABELS: Record<StorageFolderKind, { name: string; hint: string }> = {
  cache: { name: 'Cache', hint: 'Image previews, star lists and plate solves.' },
  stacks: { name: 'Stacks', hint: 'Stacks, their processing, and WBPP runs. Leave blank to use the cache folder.' },
  calibration: {
    name: 'Calibration masters',
    hint: 'Masters built from your calibration frames. Leave blank to use the cache folder.',
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
      <UnsavedMark label="storage folders" dirty={dirty} />
      <h3>Folders</h3>
      {current.folders.map((folder) => {
        const label = FOLDER_LABELS[folder.kind];
        const fixed = folder.source === 'server_config';
        const inputId = `storage-folder-${folder.kind}`;
        return (
          <div className="storage-folder" key={folder.kind}>
            <label htmlFor={inputId} className="stack-method-label">
              {label.name}
            </label>
            <PathField
              id={inputId}
              dialogTitle={`Select the ${label.name.toLowerCase()} folder`}
              value={fixed ? folder.next_path : draft[folder.kind]}
              placeholder={folder.kind === 'cache' ? folder.next_path : 'Same as the cache'}
              disabled={!editable || fixed || save.isPending}
              aria-describedby={`${inputId}-hint`}
              onChange={(path) => setDraft((previous) => ({ ...previous, [folder.kind]: path }))}
            />
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
        <p className="muted">This server has no writable settings file. Change these folders in its configuration file.</p>
      )}
      {editable && (
        <div className="storage-folder-actions">
          <button type="button" className="save-button" disabled={!dirty || save.isPending} onClick={submit}>
            {save.isPending ? 'Saving…' : 'Save folders'}
          </button>
          {pending && (
            <span className="muted">
              Restart PSF Guard to use the new folders. PSF Guard moves existing files before startup
        finishes. Moving many files may delay startup.
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

/** One limit: a slider saved when let go, on Enter, or on leaving it. */
function LimitSlider({
  label,
  ariaLabel,
  hint,
  value,
  min,
  disabled,
  resetToken,
  onCommit,
}: {
  label: string;
  ariaLabel: string;
  hint: string;
  value: number;
  min: number;
  disabled: boolean;
  /** Changes after a failed save, so the slider shows the saved value again. */
  resetToken: number;
  onCommit: (percent: number) => void;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value, resetToken]);
  const commit = () => {
    if (draft !== value) onCommit(draft);
  };
  return (
    <div className="review-preference worker-share">
      <span>
        <span className="stack-method-label">
          {label}
          <em className="worker-share-value">{draft >= 100 ? 'no limit' : `${draft}%`}</em>
        </span>
        <small>{hint}</small>
      </span>
      {/* Not disabled while saving: that would take the slider out of a keyboard user's hands. */}
      <input
        type="range"
        min={min}
        max={100}
        step={1}
        value={draft}
        disabled={disabled}
        aria-label={ariaLabel}
        onChange={(event) => setDraft(Number(event.target.value))}
        // Each save also checks the volumes, so not on every arrow press.
        onPointerUp={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter') commit();
        }}
        onBlur={commit}
      />
    </div>
  );
}

/**
 * How full each storage volume may get. Past the limit, image previews go
 * first, least recently used first, so stacks keep their room. Stacks and
 * masters may have limits of their own for the volumes they sit on.
 */
export default function StorageSettings({ canManage }: { canManage: boolean }) {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: QUERY_KEY,
    queryFn: apiClient.getStorageSettings,
    refetchInterval: 60_000,
  });
  const [failures, setFailures] = useState(0);
  const save = useMutation({
    mutationFn: apiClient.updateStorageSettings,
    onSuccess: (updated) => queryClient.setQueryData(QUERY_KEY, updated),
    onError: () => setFailures((count) => count + 1),
  });

  if (settings.isLoading) return null;
  if (settings.isError || !settings.data) {
    return (
      <div className="stack-method-settings">
        <h3>Disk use</h3>
        <p className="muted" role="alert">Could not load the disk limit.</p>
      </div>
    );
  }
  const current = settings.data;
  const separate =
    current.stack_max_volume_percent !== null || current.calibration_max_volume_percent !== null;
  // Not held back while another save runs: each names only its own limit,
  // and the server applies them one at a time.
  const commit = (limits: StorageLimitsUpdate) => save.mutate(limits);
  const needsManagement = canManage ? '' : ' Changing it needs database management on this server.';

  return (
    <div className="stack-method-settings storage-settings">
      <h3>Disk use</h3>
      <p className="muted">
        When disk usage exceeds the limit, PSF Guard first deletes the least recently viewed image
        previews, aiming for two percentage points below the limit. It then checks stack checkpoints
        at least a day old and calibration masters unused for a week. Stacks and color previews
        are never deleted automatically. PSF Guard regenerates previews when you open an image
        and rebuilds masters from their source frames when needed.
      </p>
      {current.volumes.length === 0 ? (
        <p className="muted">The first check runs a minute after the server starts.</p>
      ) : (
        current.volumes.map((volume) => <VolumeUse key={volume.path} volume={volume} />)
      )}
      <fieldset className="calibration-settings-group" disabled={!canManage}>
        <LimitSlider
          label={separate ? 'Cache volume usage limit' : 'Volume usage limit'}
          ariaLabel={
            separate
              ? 'Cache volume usage limit, in percent'
              : 'Volume usage limit, in percent'
          }
          hint={`Percentage of total disk space used, including other files. Default ${current.default_max_volume_percent}%; 100% disables automatic cleanup.${separate ? ' A volume shared with stacks or masters uses the lowest of their limits.' : ''}${needsManagement}`}
          value={current.max_volume_percent}
          min={current.min_max_volume_percent}
          disabled={!canManage}
          resetToken={failures}
          onCommit={(percent) => commit({ max_volume_percent: percent })}
        />
        <label className="review-preference storage-separate-limits">
          <input
            type="checkbox"
            checked={separate}
            disabled={!canManage || save.isPending}
            onChange={(event) =>
              commit(
                event.target.checked
                  ? {
                      stack_max_volume_percent: current.max_volume_percent,
                      calibration_max_volume_percent: current.max_volume_percent,
                    }
                  : { stack_max_volume_percent: null, calibration_max_volume_percent: null },
              )
            }
          />
          <span>
            Separate limits for stacks and calibration masters
            <small className="muted">
              {' '}
              For folders on their own volumes. A volume holding more than one folder uses the lowest
              of their limits.
            </small>
          </span>
        </label>
        {separate && (
          <>
            <LimitSlider
              label="Stack volume usage limit"
              ariaLabel="Stack volume usage limit, in percent"
              hint="PSF Guard never deletes stacks automatically. On a separate stack volume, cleanup deletes only checkpoints at least a day old. If stacks share the cache volume, the lower limit also applies to preview cleanup."
              value={current.stack_max_volume_percent ?? current.max_volume_percent}
              min={current.min_max_volume_percent}
              disabled={!canManage}
              resetToken={failures}
              onCommit={(percent) => commit({ stack_max_volume_percent: percent })}
            />
            <LimitSlider
              label="Calibration master volume usage limit"
              ariaLabel="Calibration master volume usage limit, in percent"
              hint="Above this limit, PSF Guard deletes masters unused for a week, least recently used first, but only if cleanup can bring usage below the limit. It preserves masters whose source frames are missing. If masters share the cache volume, the lower limit also applies to preview cleanup."
              value={current.calibration_max_volume_percent ?? current.max_volume_percent}
              min={current.min_max_volume_percent}
              disabled={!canManage}
              resetToken={failures}
              onCommit={(percent) => commit({ calibration_max_volume_percent: percent })}
            />
          </>
        )}
      </fieldset>
      {save.isError && (
        <p className="error-text" role="alert">{(save.error as Error).message}</p>
      )}
      <StorageFolders current={current} canManage={canManage} />
    </div>
  );
}
