import { useId, useState, type ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { BookOpen, ChevronDown, ChevronRight, Telescope, Trash2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { CalibrationRigSettings } from '../../api/types';
import { useAccess } from '../../auth/access';
import { useAllDatabases } from '../../hooks/useDatabases';
import { isTauriApp } from '../../utils/tauri';
import DatabaseRemoveDialog from '../DatabaseRemoveDialog';
import { openSettings } from '../../utils/settingsIntent';
import { formatDegrees } from './framingModel';
import RigProfileCard, { type RigSetupTab } from './RigProfileCard';
import EarlyBeta from './EarlyBeta';
import { retryWhenBusy } from './retry';

function describeStatus(payload: Record<string, unknown>, at: number): string {
  const phase = typeof payload.phase === 'string' ? payload.phase : typeof payload.state === 'string' ? payload.state : 'reported';
  const wait = typeof payload.wait_reason === 'string' && payload.wait_reason ? `, ${payload.wait_reason}` : '';
  return `${phase}${wait}, ${new Date(at).toLocaleString()}`;
}

/** Every registered database as a rig: planning state, optics and last word from its plugin. */
export default function DirectorRigs({ databaseId, syncPlugin, initialTab }: {
  databaseId?: string;
  /** The opened database's Sync plugin section, for its Plugins tab. */
  syncPlugin?: ReactNode;
  initialTab?: RigSetupTab;
} = {}) {
  const id = useId();
  const databases = useAllDatabases();
  const [openSetup, setOpenSetup] = useState<string | null>(null);
  // A setup once opened stays mounted, hidden while folded, so folding it
  // keeps its unsaved edits (and Settings still asks before closing).
  const [opened, setOpened] = useState<ReadonlySet<string>>(() => new Set());
  const profiles = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, retry: retryWhenBusy, retryDelay: 1200, refetchInterval: 30_000 });
  // Only needed to name a peer; most servers have none.
  const peers = useQuery({ queryKey: ['peers'], queryFn: apiClient.getPeers, retry: false, refetchOnWindowFocus: false, enabled: profiles.data?.some(entry => entry.profile?.peer_id) ?? false });
  // The calibration library's rigs (a camera and its frame size) each
  // catalog records: its row links to their calibration settings.
  const calibration = useQuery({ queryKey: ['calibration-settings'], queryFn: apiClient.getCalibrationSettings, retry: false, refetchOnWindowFocus: false });
  // Removing a database needs database management as well as an editor's account.
  const { canWrite } = useAccess();
  const { data: serverInfo } = useQuery({ queryKey: ['serverInfo'], queryFn: apiClient.getServerInfo, staleTime: 5 * 60 * 1000 });
  const canRemove = canWrite && (isTauriApp() || (serverInfo?.allow_database_management ?? false));
  const [removing, setRemoving] = useState<{ id: string; name: string } | null>(null);
  return <><section aria-label="Rig databases" className="director-records director-rigs">
    {!databaseId && <div className="director-toolbar"><div className="early-beta-heading"><h2>Rigs</h2><EarlyBeta /></div></div>}
    {databases.isPending && <p role="status">Loading rig databases...</p>}
    {databases.isError && <div role="alert"><p>{databases.error.message}</p><button type="button" onClick={() => void databases.refetch()}>Retry</button></div>}
    {databases.data?.length === 0 && <button type="button" onClick={() => openSettings('add')}>Add database</button>}
    <ul className="director-list director-rig-list">{databases.data?.filter(db => !databaseId || db.id === databaseId).map(db => {
      const profile = profiles.data?.find(entry => entry.catalog_slug === db.id);
      const status = profile && statuses.data?.find(entry => entry.rig.id === profile.rig.id);
      const expanded = !!databaseId || openSetup === db.id;
      const setupId = `${id}-${db.id}-setup`;
      return <li key={db.id} className={`director-rig${expanded ? ' is-open' : ''}`}>
        <div className="director-record director-record-wide">
          <div className="director-record-name">
            <h3 className="director-rig-title"><Telescope size={20} aria-hidden="true" />{db.name}</h3>
            <span className="director-muted">{!profiles.data ? (profiles.isError ? 'Could not load rig settings' : 'Checking rig...') : !profile ? (expanded ? 'Setting up the rig…' : 'No rig yet. Click Setup to create it.')
              : profile.field_of_view ? `Field ${formatDegrees(profile.field_of_view.width_degrees)} × ${formatDegrees(profile.field_of_view.height_degrees)}, ${profile.field_of_view.pixel_scale_arcsec.toFixed(2)}″/px${profile.profile?.configuration ? ', camera reported' : ', camera not reported yet'}`
              : 'Field of view not set'}</span>
            {profile?.profile?.peer_id && <span className="director-muted">Plans push to {peers.data?.find(peer => peer.id === profile.profile?.peer_id)?.name ?? profile.profile.peer_id}</span>}
            {status?.status && <span className="director-muted">Director plugin: {describeStatus(status.status.payload, status.status.reported_at_ms)}</span>}
            <CalibrationRigLinks rigs={calibration.data?.rigs?.filter(rig => rig.database_ids?.includes(db.id)) ?? []} />
          </div>
          <Link to={`/?${new URLSearchParams({ db: db.id, dbfilter: db.id })}`}><BookOpen size={16} aria-hidden="true" />Library</Link>
          {canRemove && <button type="button" aria-label={`Remove ${db.name}`} title={`Remove ${db.name} from PSF Guard; its files stay on disk`} onClick={() => setRemoving({ id: db.id, name: db.name })}><Trash2 size={16} aria-hidden="true" />Remove</button>}
          {!databaseId && <button type="button" aria-expanded={expanded} aria-controls={setupId} aria-label={`Setup ${db.name}`} title={`Setup ${db.name}`} onClick={() => { setOpenSetup(expanded ? null : db.id); setOpened(current => current.has(db.id) ? current : new Set(current).add(db.id)); }}>{expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}Setup</button>}
        </div>
        <div id={setupId} hidden={!expanded}>{(expanded || opened.has(db.id)) && <RigProfileCard key={db.id} slug={db.id} syncPlugin={db.id === databaseId ? syncPlugin : undefined} initialTab={db.id === databaseId ? initialTab : undefined} />}</div>
      </li>;
    })}</ul>
    {removing && <DatabaseRemoveDialog database={removing} onClose={() => setRemoving(null)} onRemoved={() => setRemoving(null)} />}
  </section>
  </>;
}

/** A catalog's calibration rigs, each a link into its calibration settings
 *  under Settings › Calibration, with how many values it sets itself. */
function CalibrationRigLinks({ rigs }: { rigs: CalibrationRigSettings[] }) {
  if (rigs.length === 0) return null;
  return <span className="director-rig-calibration">
    <span className="director-muted">Calibration:</span>
    {rigs.map(rig => {
      const own = Object.keys(rig.rules).length;
      return <button key={rig.rig_uuid} type="button" className="director-link-button"
        title={`Calibration settings for ${rig.name}${rig.detail ? ` (${rig.detail})` : ''}`}
        onClick={() => openSettings('calibration', { rig: { uuid: rig.rig_uuid, name: rig.name } })}>
        {rig.name}{rig.detail ? ` · ${rig.detail}` : ''}{own > 0 ? ` · ${own} own value${own === 1 ? '' : 's'}` : ''}
      </button>;
    })}
  </span>;
}
