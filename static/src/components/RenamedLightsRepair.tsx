import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';

const EXAMPLES = 5;

function plural(count: number, one: string, many: string): string {
  return `${count.toLocaleString()} ${count === 1 ? one : many}`;
}

function baseName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/** Lights an import catalogued twice because their file was renamed after
 *  it was imported. The first light of each pair shows as Catalog only,
 *  since its file is gone. Silent when there are none. */
export default function RenamedLightsRepair({ dbId, dbName, canManage }: { dbId: string; dbName: string; canManage: boolean }) {
  const client = useQueryClient();
  const report = useQuery({ queryKey: ['db', dbId, 'renamed-lights'], queryFn: () => apiClient.getRenamedLights(dbId), staleTime: 60_000, retry: false });
  const merge = useMutation({
    retry: false,
    mutationFn: () => apiClient.mergeRenamedLights(dbId),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ['db', dbId, 'renamed-lights'] });
      void client.invalidateQueries({ queryKey: ['db', dbId] });
    },
  });
  if (merge.isSuccess) {
    return <div className="settings-note" role="status">
      {merge.data.backup_path
        ? <>Merged {plural(merge.data.merged.length, 'renamed light', 'renamed lights')} in {dbName}. The database was copied first to <code>{merge.data.backup_path}</code>.</>
        : <>{dbName} has no renamed lights to merge; nothing was changed.</>}
    </div>;
  }
  const data = report.data;
  if (!data || data.pairs.length + data.held.length === 0) return null;
  const pairs = data.pairs.length;
  return <div className="settings-note settings-warning" role="note" aria-label={`Renamed lights in ${dbName}`}>
    <p>
      <strong>{plural(data.pairs.length + data.held.length, 'light was', 'lights were')} imported again after the file was renamed.</strong>{' '}
      The first light of each pair shows as Catalog only, because its file is gone.
    </p>
    {pairs > 0 && <ul>
      {data.pairs.slice(0, EXAMPLES).map(pair => <li key={pair.twin.id}>
        <code>{baseName(pair.kept.file_name)}</code> → <code>{baseName(pair.twin.file_name)}</code>
      </li>)}
      {pairs > EXAMPLES && <li>and {plural(pairs - EXAMPLES, 'other', 'others')}</li>}
    </ul>}
    {data.held.length > 0 && <p>
      {plural(data.held.length, 'pair stays', 'pairs stay')} as {data.held.length === 1 ? 'it is' : 'they are'}:{' '}
      {data.held.slice(0, EXAMPLES).map(held => `${baseName(held.twin.file_name)} (${held.reason})`).join('; ')}.
    </p>}
    {pairs > 0 && (canManage && data.writable
      ? <p>
          <button type="button" className="browse-button" disabled={merge.isPending} onClick={() => merge.mutate()}>
            {merge.isPending ? 'Merging…' : 'Merge renamed lights'}
          </button>{' '}
          Copies the database beside itself first. Each first light then uses the renamed file and keeps its grade,
          or takes the second light's grade when it had none, and the second light is removed. Close N.I.N.A. on
          that rig before merging.
        </p>
      : <p>
          {canManage ? 'This server cannot write the file. ' : 'An editor can merge them, or '}
          Run <code>psf-guard merge-renamed-lights --db {dbId} --apply</code> where the file is writable.
        </p>)}
    {merge.isError && <p className="settings-error" role="alert">{merge.error instanceof Error ? merge.error.message : 'The merge failed.'}</p>}
  </div>;
}
