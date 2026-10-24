import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  PixInsightSettings,
  StorageFolderKind,
  StorageFoldersUpdate,
  StorageLimitsUpdate,
  StorageSettings,
  WbppStoragePolicy,
} from '../api/types';
import { DEFAULT_WBPP_STORAGE, validWbppStorage } from '../utils/storageSettings';

export const STORAGE_QUERY_KEY = ['storage-settings'] as const;
export const PIXINSIGHT_QUERY_KEY = ['pixinsight-settings'] as const;
const FOLDER_FIELDS = {
  cache: 'cache_dir',
  stacks: 'stack_dir',
  calibration: 'calibration_dir',
} as const;
type Limits = Pick<
  StorageSettings,
  'max_volume_percent' | 'stack_max_volume_percent' | 'calibration_max_volume_percent'
>;

/** One draft and one save for limits, folders, and WBPP retention. */
export function useStorageSettingsForm(current: StorageSettings, pixinsight: PixInsightSettings) {
  const client = useQueryClient();
  const [limitDraft, setLimitDraft] = useState<Partial<Limits>>({});
  const [folderDraft, setFolderDraft] = useState<Partial<Record<StorageFolderKind, string>>>({});
  const [wbppDraft, setWbppDraft] = useState<Partial<WbppStoragePolicy>>({});
  const [wbppFolderDraft, setWbppFolderDraft] = useState<string | null>(null);
  const limits: Limits = {
    max_volume_percent: current.max_volume_percent,
    stack_max_volume_percent: current.stack_max_volume_percent,
    calibration_max_volume_percent: current.calibration_max_volume_percent,
    ...limitDraft,
  };
  const folders: Record<StorageFolderKind, string> = { cache: '', stacks: '', calibration: '' };
  for (const folder of current.folders)
    folders[folder.kind] = folderDraft[folder.kind] ?? folder.chosen ?? '';
  const savedWbpp = { ...DEFAULT_WBPP_STORAGE, ...pixinsight.storage };
  const wbpp = { ...savedWbpp, ...wbppDraft };
  const wbppFolder = wbppFolderDraft ?? pixinsight.runs_dir ?? '';

  const limitUpdate: StorageLimitsUpdate = {};
  if (limits.max_volume_percent !== current.max_volume_percent)
    limitUpdate.max_volume_percent = limits.max_volume_percent;
  if (limits.stack_max_volume_percent !== current.stack_max_volume_percent)
    limitUpdate.stack_max_volume_percent = limits.stack_max_volume_percent;
  if (limits.calibration_max_volume_percent !== current.calibration_max_volume_percent)
    limitUpdate.calibration_max_volume_percent = limits.calibration_max_volume_percent;
  const folderUpdate: StorageFoldersUpdate = {};
  for (const folder of current.folders) {
    if (folder.source !== 'server_config' && folders[folder.kind].trim() !== (folder.chosen ?? '')) {
      folderUpdate[FOLDER_FIELDS[folder.kind]] = folders[folder.kind].trim();
    }
  }
  const wbppChanged =
    wbppFolder.trim() !== (pixinsight.runs_dir ?? '') || JSON.stringify(wbpp) !== JSON.stringify(savedWbpp);
  const limitsChanged = Object.keys(limitUpdate).length > 0;
  const foldersChanged = Object.keys(folderUpdate).length > 0;
  const dirty = limitsChanged || foldersChanged || wbppChanged;

  const save = useMutation({
    mutationFn: async () => {
      const remember = async (key: readonly string[], value: unknown) => {
        // A periodic read may also begin while a save request is in flight.
        // Cancel it before publishing that request's saved response.
        await client.cancelQueries({ queryKey: key });
        client.setQueryData(key, value);
      };
      await Promise.all([
        client.cancelQueries({ queryKey: STORAGE_QUERY_KEY }),
        client.cancelQueries({ queryKey: PIXINSIGHT_QUERY_KEY }),
      ]);
      let saved = false;
      try {
        // Update each saved query immediately. If a later request fails, the
        // draft remains and another save sends only the outstanding changes.
        if (foldersChanged) {
          await remember(STORAGE_QUERY_KEY, await apiClient.updateStorageFolders(folderUpdate));
          saved = true;
        }
        if (wbppChanged) {
          await remember(
            PIXINSIGHT_QUERY_KEY,
            await apiClient.updatePixInsightSettings({
              binary: pixinsight.binary,
              runs_dir: wbppFolder.trim() || null,
              storage: wbpp,
            })
          );
          saved = true;
        }
        if (limitsChanged) {
          await remember(STORAGE_QUERY_KEY, await apiClient.updateStorageSettings(limitUpdate));
        }
      } catch (error) {
        throw new Error(
          `${saved ? 'Some settings were saved. ' : ''}${(error as Error).message} Your remaining edits are still shown.`,
          { cause: error }
        );
      }
    },
    onSuccess: () => {
      setLimitDraft({});
      setFolderDraft({});
      setWbppDraft({});
      setWbppFolderDraft(null);
    },
    onError: () => {
      void client.invalidateQueries({ queryKey: STORAGE_QUERY_KEY });
      void client.invalidateQueries({ queryKey: PIXINSIGHT_QUERY_KEY });
    },
  });

  return {
    limits,
    folders,
    wbpp,
    wbppFolder,
    save,
    dirty,
    valid: validWbppStorage(wbpp),
    editLimits: (update: Partial<Limits>) => setLimitDraft((previous) => ({ ...previous, ...update })),
    editFolder: (kind: StorageFolderKind, value: string) =>
      setFolderDraft((previous) => ({ ...previous, [kind]: value })),
    editWbpp: (field: keyof WbppStoragePolicy, value: number) =>
      setWbppDraft((previous) => ({ ...previous, [field]: value })),
    editWbppFolder: setWbppFolderDraft,
  };
}

export type StorageSettingsForm = ReturnType<typeof useStorageSettingsForm>;
