import { useQuery } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../../api/client';
import type { DirectorRigMode } from '../../api/directorTypes';
import { useAccess } from '../../auth/access';
import '../director/DirectorPage.css';

const MODE_LABEL = { target_scheduler: 'Target Scheduler', director: 'Director' } as const;

/** Which N.I.N.A. plugins serve this rig, and whether each is on: the first
 *  thing the Plugins tab shows. Director appears only with planning. */
export default function PluginsOnRig({ databaseId, director }: {
  databaseId: string;
  director?: { mode: DirectorRigMode; onChangeMode: () => void };
}) {
  const { canWrite } = useAccess();
  const databases = useQuery({ queryKey: ['databases'], queryFn: apiClient.getDatabases, staleTime: 0, refetchOnWindowFocus: false });
  const plugin = useQuery({
    queryKey: ['directorPlugin', databaseId],
    queryFn: () => apiClient.getDirectorPlugin(databaseId),
    enabled: !!director && canWrite,
    staleTime: 0,
    refetchOnWindowFocus: false,
    // The Director panel shares this query: the store answers 503 while
    // busy, which means wait, as there.
    retry: (count, error) => count < 5 && isAxiosError(error) && error.response?.status === 503,
    retryDelay: 700,
  });
  const upload = databases.data?.find(entry => entry.id === databaseId)?.remote_image_upload;
  // A paired plugin, or the remote API key a script uses.
  const paired = upload?.token_configured ?? false;
  const syncOn = paired && !!upload?.sync_enabled;
  const sync = !paired ? 'Not paired' : syncOn ? 'scheduler sync' : 'Paired, scheduler sync off';
  // Uploads serve both plugins, so they have their own row.
  const uploadsOn = !!upload?.enabled;
  const view = plugin.data;
  const directorOn = !!view && view.clients.length > 0 && view.work === 'ready';
  const directorState = !canWrite
    ? 'Shown to editors'
    : !view
    ? (plugin.isError ? 'Could not be read' : 'Checking...')
    : view.clients.length === 0
      ? 'Not paired'
      : view.work === 'ready' ? 'Work allowed' : 'Paired, work off';
  return <fieldset className="plugins-on-rig">
    <legend>Plugins on this rig</legend>
    <ul aria-label="Plugins on this rig">
      <li data-testid="plugin-sync">
        <span>PSF Guard Sync</span>
        <span className="plugin-state" data-on={syncOn}>{syncOn ? 'On' : 'Off'}</span>
        <span className="director-muted">{sync}</span>
      </li>
      <li data-testid="plugin-uploads">
        <span>Image uploads</span>
        <span className="plugin-state" data-on={uploadsOn}>{uploadsOn ? 'On' : 'Off'}</span>
        <span className="director-muted">{uploadsOn ? `to ${upload?.image_directory}` : 'Neither plugin sends image files'}</span>
      </li>
      {director && <li data-testid="plugin-director">
        <span>PSF Guard Director</span>
        {canWrite ? <span className="plugin-state" data-on={directorOn}>{directorOn ? 'On' : 'Off'}</span> : <span />}
        <span className="director-muted">{directorState}</span>
      </li>}
    </ul>
    {director && <>
      <p>Picks the work: {MODE_LABEL[director.mode]}. <button type="button" className="link-button" onClick={director.onChangeMode}>Change mode</button></p>
      <p className="director-muted">Both plugins can stay installed. Sync moves images and catalog data in either mode.</p>
    </>}
  </fieldset>;
}
