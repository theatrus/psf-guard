import { useIsMutating, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DatabaseSummary, RemoteImageUploadPlacement, RemoteImageUploadSummary } from '../../api/types';

/** A change to the database's receiver: what the Sync and Director plugins
 *  may send it. Fields left out keep their stored value. */
export type ReceiverChange = {
  enabled?: boolean;
  image_directory?: string;
  token?: string;
  sync_enabled?: boolean;
  placement?: RemoteImageUploadPlacement;
  directory_template?: string;
  directory_template_source?: 'catalog' | 'preset';
  rescan_directory_layout?: boolean;
};

export const requestMessage = (error: unknown) => error instanceof Error ? error.message : 'The request failed';

/** What the database's receiver keeps, as an update that changes nothing. */
function unchanged(upload: RemoteImageUploadSummary | undefined) {
  return {
    enabled: upload?.enabled ?? false,
    image_directory: upload?.image_directory,
    sync_enabled: upload?.sync_enabled ?? false,
    placement: upload?.placement,
  };
}

/** The database's receiver settings, and a save that sends one change with
 *  everything else unchanged. Shared by the Sync and Image uploads panels. */
export function useRemoteReceiver(databaseId: string, options: { onChanged?: () => void; pollMs?: number | false } = {}) {
  const client = useQueryClient();
  const loaded = useQuery({
    queryKey: ['databases'],
    queryFn: apiClient.getDatabases,
    refetchInterval: options.pollMs ?? false,
    staleTime: 0,
    refetchOnWindowFocus: true,
  });
  const database: DatabaseSummary | undefined = loaded.data?.find(entry => entry.id === databaseId);
  const upload = database?.remote_image_upload;
  const mutationKey = ['remoteReceiver', databaseId];
  const act = useMutation({
    mutationKey,
    retry: false,
    mutationFn: (work: () => Promise<unknown>) => work(),
    onSettled: () => { void client.invalidateQueries({ queryKey: ['databases'] }); options.onChanged?.(); },
  });
  const save = (change: ReceiverChange, onSaved?: () => void) => act.mutate(() => apiClient.updateDatabase(databaseId, {
    remote_image_upload: { ...unchanged(upload), ...change },
  }), { onSuccess: () => onSaved?.() });
  // The Sync and Image uploads cards each save the whole receiver. While
  // either saves, both wait, and the next save starts from fresh settings,
  // so one card never undoes the other.
  const busy = useIsMutating({ mutationKey }) > 0 || loaded.isFetching;
  return { loaded, database, upload, act, save, busy };
}
