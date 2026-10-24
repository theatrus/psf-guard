import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  CacheVolumeReport,
  StorageFolderKind,
  StorageSettings as StorageSettingsData,
  PixInsightSettings,
} from '../api/types';
import { isTauriApp, tauriConfig } from '../utils/tauri';
import PathField from './PathField';
import UnsavedMark from './UnsavedMark';
import WbppStorageSettings, { WbppStorageReport } from './WbppStorageSettings';
import {
  useStorageSettingsForm,
  STORAGE_QUERY_KEY,
  PIXINSIGHT_QUERY_KEY,
  type StorageSettingsForm,
} from './useStorageSettingsForm';

const QUERY_KEY = STORAGE_QUERY_KEY;

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
        Stacks {bytes(volume.stack_bytes)} · calibration masters {bytes(volume.calibration_bytes)} · image
        previews {bytes(volume.preview_bytes)}
        {volume.other_bytes > 0 && <> · other {bytes(volume.other_bytes)}</>}
        {volume.databases.length > 0 && <> · for {volume.databases.join(', ')}</>}
        {volume.kinds.length > 0 && <> · used for {kindList(volume.kinds)}</>}
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
  stacks: { name: 'Stacks', hint: 'Stacks and their processing. Leave blank to use the cache folder.' },
  calibration: {
    name: 'Calibration masters',
    hint: 'Masters built from your calibration frames. Leave blank to use the cache folder.',
  },
};

/** Folder changes for cache, stacks and calibration take effect after a restart.
 * WBPP changes apply to new runs as soon as the shared form is saved. */
function StorageFolders({
  current,
  canManage,
  form,
}: {
  current: StorageSettingsData;
  canManage: boolean;
  form: StorageSettingsForm;
}) {
  const [restarting, setRestarting] = useState(false);
  const pending = current.folders.some((folder) => folder.path !== folder.next_path);
  const editable = canManage && current.can_choose_folders && !form.save.isPending;
  const restart = async () => {
    setRestarting(true);
    if (!(await tauriConfig.restartApplication())) setRestarting(false);
  };
  return (
    <div className="storage-folders">
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
              value={fixed ? folder.next_path : form.folders[folder.kind]}
              placeholder={folder.kind === 'cache' ? folder.next_path : 'Same as the cache'}
              disabled={!editable || fixed}
              aria-describedby={`${inputId}-hint`}
              onChange={(path) => form.editFolder(folder.kind, path)}
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
      <div className="storage-folder">
        <label htmlFor="storage-folder-wbpp" className="stack-method-label">
          WBPP runs
        </label>
        <PathField
          id="storage-folder-wbpp"
          aria-label="WBPP storage folder"
          dialogTitle="Select the WBPP runs folder"
          value={form.wbppFolder}
          placeholder="Export directory or stack folder"
          disabled={!editable}
          aria-describedby="storage-folder-wbpp-hint"
          onChange={form.editWbppFolder}
        />
        <small className="muted" id="storage-folder-wbpp-hint">
          WBPP scripts and output. New runs use this folder after you save; old folders remain managed. Leave
          blank to use each database’s export directory, or its stack folder.
        </small>
      </div>
      {current.folder_notes.map((note) => (
        <p key={note} className="muted storage-volume-note">
          {note}
        </p>
      ))}
      {!current.can_choose_folders && (
        <p className="muted">
          This server has no writable settings file. Change these folders in its configuration file.
        </p>
      )}
      {pending && (
        <p className="muted">
          Restart PSF Guard to use the new folders. PSF Guard moves existing files before startup finishes.
          Moving many files may delay startup.
        </p>
      )}
      {pending && editable && isTauriApp() && (
        <button type="button" className="btn btn-secondary" onClick={restart} disabled={restarting}>
          {restarting ? 'Restarting…' : 'Restart now'}
        </button>
      )}
    </div>
  );
}

/** A staged limit; the shared Save button applies it. */
function LimitSlider({
  label,
  ariaLabel,
  hint,
  value,
  min,
  disabled,
  onChange,
}: {
  label: string;
  ariaLabel: string;
  hint: string;
  value: number;
  min: number;
  disabled: boolean;
  onChange: (percent: number) => void;
}) {
  return (
    <div className="review-preference worker-share">
      <span>
        <span className="stack-method-label">
          {label}
          <em className="worker-share-value">{value >= 100 ? 'no limit' : `${value}%`}</em>
        </span>
        <small>{hint}</small>
      </span>
      <input
        type="range"
        min={min}
        max={100}
        step={1}
        value={value}
        disabled={disabled}
        aria-label={ariaLabel}
        onChange={(event) => onChange(Number(event.target.value))}
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
  const settings = useQuery({
    queryKey: QUERY_KEY,
    queryFn: apiClient.getStorageSettings,
    refetchInterval: 60_000,
  });
  const pixinsight = useQuery({
    queryKey: PIXINSIGHT_QUERY_KEY,
    queryFn: apiClient.getPixInsightSettings,
    refetchInterval: 60_000,
  });
  if (settings.isLoading || pixinsight.isLoading) return null;
  if (settings.isError || !settings.data || pixinsight.isError || !pixinsight.data) {
    return (
      <div className="stack-method-settings">
        <h3>Disk use</h3>
        <p className="muted" role="alert">
          Could not load storage settings.
        </p>
      </div>
    );
  }
  return <StorageForm current={settings.data} pixinsight={pixinsight.data} canManage={canManage} />;
}

function StorageForm({
  current,
  pixinsight,
  canManage,
}: {
  current: StorageSettingsData;
  pixinsight: PixInsightSettings;
  canManage: boolean;
}) {
  const form = useStorageSettingsForm(current, pixinsight);
  const { save, limits } = form;
  const separate = limits.stack_max_volume_percent !== null || limits.calibration_max_volume_percent !== null;
  const commit = form.editLimits;
  const needsManagement = canManage ? '' : ' Changing it needs database management on this server.';

  return (
    <div className="stack-method-settings storage-settings">
      <h3>Disk use</h3>
      <UnsavedMark label="storage settings" dirty={form.dirty} />
      <p className="muted">
        When disk usage exceeds the limit, PSF Guard first deletes the least recently viewed image previews,
        aiming for two percentage points below the limit. It then checks stack checkpoints at least a day old
        and calibration masters unused for a week. Stacks and color previews are never deleted automatically.
        PSF Guard regenerates previews when you open an image and rebuilds masters from their source frames
        when needed.
      </p>
      {current.volumes.length === 0 ? (
        <p className="muted">The first check runs a minute after the server starts.</p>
      ) : (
        current.volumes.map((volume) => <VolumeUse key={volume.path} volume={volume} />)
      )}
      <p className="muted">
        Click <strong>Save storage settings</strong> to apply limits and WBPP options. Cache, stack, and
        calibration folder changes need a restart.
      </p>
      <fieldset className="calibration-settings-group" disabled={!canManage || save.isPending}>
        <legend>Limits</legend>
        <LimitSlider
          label={separate ? 'Cache volume usage limit' : 'Volume usage limit'}
          ariaLabel={separate ? 'Cache volume usage limit, in percent' : 'Volume usage limit, in percent'}
          hint={`Percentage of total disk space used, including other files. Default ${current.default_max_volume_percent}%; 100% disables automatic cleanup.${separate ? ' A volume shared with stacks or masters uses the lowest of their limits.' : ''}${needsManagement}`}
          value={limits.max_volume_percent}
          min={current.min_max_volume_percent}
          disabled={!canManage}
          onChange={(percent) => commit({ max_volume_percent: percent })}
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
                      stack_max_volume_percent: limits.max_volume_percent,
                      calibration_max_volume_percent: limits.max_volume_percent,
                    }
                  : { stack_max_volume_percent: null, calibration_max_volume_percent: null }
              )
            }
          />
          <span>
            Separate limits for stacks and calibration masters
            <small className="muted">
              {' '}
              For folders on their own volumes. A volume holding more than one folder uses the lowest of their
              limits.
            </small>
          </span>
        </label>
        {separate && (
          <>
            <LimitSlider
              label="Stack volume usage limit"
              ariaLabel="Stack volume usage limit, in percent"
              hint="PSF Guard never deletes stacks automatically. On a separate stack volume, cleanup deletes only checkpoints at least a day old. If stacks share the cache volume, the lower limit also applies to preview cleanup."
              value={limits.stack_max_volume_percent ?? limits.max_volume_percent}
              min={current.min_max_volume_percent}
              disabled={!canManage}
              onChange={(percent) => commit({ stack_max_volume_percent: percent })}
            />
            <LimitSlider
              label="Calibration master volume usage limit"
              ariaLabel="Calibration master volume usage limit, in percent"
              hint="Above this limit, PSF Guard deletes masters unused for a week, least recently used first, but only if cleanup can bring usage below the limit. It preserves masters whose source frames are missing. If masters share the cache volume, the lower limit also applies to preview cleanup."
              value={limits.calibration_max_volume_percent ?? limits.max_volume_percent}
              min={current.min_max_volume_percent}
              disabled={!canManage}
              onChange={(percent) => commit({ calibration_max_volume_percent: percent })}
            />
          </>
        )}
        <LimitSlider
          label="WBPP volume usage limit"
          ariaLabel="WBPP volume usage limit, in percent"
          hint="Total volume usage, including other files. A shared volume uses the lower storage limit. Cleanup removes intermediate files first; masters keep their retention period. 100% disables pressure cleanup."
          value={form.wbpp.max_volume_percent}
          min={0}
          disabled={!canManage || save.isPending}
          onChange={(percent) => form.editWbpp('max_volume_percent', percent)}
        />
      </fieldset>
      <StorageFolders current={current} canManage={canManage} form={form} />
      <WbppStorageSettings
        policy={form.wbpp}
        onChange={form.editWbpp}
        disabled={!canManage || save.isPending}
      />
      <WbppStorageReport report={pixinsight.storage_report} />
      <button
        type="button"
        className="save-button"
        disabled={!canManage || !form.dirty || !form.valid || save.isPending}
        onClick={() => save.mutate()}
      >
        {save.isPending ? 'Saving…' : 'Save storage settings'}
      </button>
      {save.isError && (
        <p className="error-text" role="alert">
          {(save.error as Error).message}
        </p>
      )}
    </div>
  );
}
