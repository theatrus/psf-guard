import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';

const TABLE_LABEL: Record<string, string> = {
  project: 'projects', target: 'targets', exposureplan: 'exposure plans', exposuretemplate: 'exposure templates',
  acquiredimage: 'frames', profilepreference: 'profile preferences',
};

/** Rows Target Scheduler's own upgrade left without a GUID. Planning, sync
 *  and the Library's multi-rig families identify projects by GUID, so such
 *  projects cannot be planned. Silent when every row has one. */
export default function MissingGuidRepair({ dbId, dbName, canManage }: { dbId: string; dbName: string; canManage: boolean }) {
  const client = useQueryClient();
  const report = useQuery({ queryKey: ['db', dbId, 'missing-guids'], queryFn: () => apiClient.getMissingGuids(dbId), staleTime: 60_000, retry: false });
  const fill = useMutation({
    retry: false,
    mutationFn: () => apiClient.fillMissingGuids(dbId),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ['db', dbId, 'missing-guids'] });
      void client.invalidateQueries({ queryKey: ['db', dbId] });
      void client.invalidateQueries({ queryKey: ['directorPlans'] });
    },
  });
  if (fill.isSuccess) {
    return <div className="settings-note" role="status">
      {fill.data.backup_path
        ? <>Gave {fill.data.filled.toLocaleString()} rows in {dbName} a GUID. The database was copied first to <code>{fill.data.backup_path}</code>.</>
        : <>Every row in {dbName} already has a GUID; nothing was changed.</>}
    </div>;
  }
  const data = report.data;
  if (!data || data.missing === 0) return null;
  const parts = data.tables.filter(gap => gap.missing > 0).map(gap => `${gap.missing.toLocaleString()} ${TABLE_LABEL[gap.table] ?? gap.table}`);
  return <div className="settings-note settings-warning" role="note" aria-label={`Missing GUIDs in ${dbName}`}>
    <p>
      <strong>{data.missing.toLocaleString()} rows have no Target Scheduler GUID</strong> ({parts.join(', ')}).
      Target Scheduler meant to add them when it upgraded this database, but skipped them. Without one a project cannot be
      planned or synced, and a multi-rig project is not recognised as one.
    </p>
    {canManage && data.writable
      ? <p>
          <button type="button" className="browse-button" disabled={fill.isPending} onClick={() => fill.mutate()}>
            {fill.isPending ? 'Filling in…' : 'Fill in GUIDs'}
          </button>{' '}
          Copies the database beside itself first, then gives each of those rows a new GUID and touches nothing else.
          Close N.I.N.A. on that rig before filling.
        </p>
      : <p>
          {canManage ? 'This server cannot write the file. ' : 'Start the server with database management to fill them in, or '}
          Run <code>psf-guard -d &lt;file&gt; fill-guids</code> where the file is writable.
        </p>}
    {fill.isError && <p className="settings-error" role="alert">{fill.error instanceof Error ? fill.error.message : 'The fill failed.'}</p>}
  </div>;
}
