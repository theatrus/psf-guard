import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAllDatabases } from '../../hooks/useDatabases';
import { openSettings } from '../../utils/settingsIntent';
import { formatDegrees } from './framingModel';
import RigProfileCard from './RigProfileCard';

function describeStatus(payload: Record<string, unknown>, at: number): string {
  const phase = typeof payload.phase === 'string' ? payload.phase : typeof payload.state === 'string' ? payload.state : 'reported';
  const wait = typeof payload.wait_reason === 'string' && payload.wait_reason ? `, ${payload.wait_reason}` : '';
  return `${phase}${wait}, ${new Date(at).toLocaleString()}`;
}

/** Every registered database as a rig: planning state, optics and last word from its plugin. */
export default function DirectorRigs() {
  const databases = useAllDatabases();
  const [openSetup, setOpenSetup] = useState<string | null>(null);
  const profiles = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: false, refetchOnWindowFocus: false });
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, retry: false, refetchInterval: 30_000 });
  return <section aria-label="Rig databases" className="director-records">
    <div className="director-toolbar"><h2>Rigs</h2></div>
    <p className="director-muted">Each registered database is one rig, and its projects are plans. Setup holds the rig's optics, site and limits; the Director plugin reports the rest.</p>
    {databases.isPending && <p role="status">Loading rig databases...</p>}
    {databases.isError && <div role="alert"><p>{databases.error.message}</p><button type="button" onClick={() => void databases.refetch()}>Retry</button></div>}
    {databases.data?.length === 0 && <button type="button" onClick={() => openSettings('add')}>Add database</button>}
    <ul className="director-list">{databases.data?.map(db => {
      const profile = profiles.data?.find(entry => entry.catalog_slug === db.id);
      const status = profile && statuses.data?.find(entry => entry.rig.id === profile.rig.id);
      return <li key={db.id}>
        <div className="director-record director-record-wide">
          <div className="director-record-name">
            <strong>{db.name}</strong>
            <span className="director-muted">{!profiles.data ? (profiles.isError ? 'Rig state unavailable' : 'Checking rig...') : !profile ? 'Not yet a rig; open Plans once to adopt it'
              : profile.field_of_view ? `Field ${formatDegrees(profile.field_of_view.width_degrees)} × ${formatDegrees(profile.field_of_view.height_degrees)}, ${profile.field_of_view.pixel_scale_arcsec.toFixed(2)}″/px${profile.profile?.configuration ? ', camera reported' : ', camera not reported yet'}`
              : 'Planning enabled, no optics yet'}</span>
            {status && <span className="director-muted">Plugin: {describeStatus(status.status.payload, status.status.reported_at_ms)}</span>}
          </div>
          <Link to={`/?${new URLSearchParams({ db: db.id, dbfilter: db.id })}`}>Overview</Link>
          <button type="button" aria-expanded={openSetup === db.id} aria-label={`Setup ${db.name}`} title={`Setup ${db.name}`} onClick={() => setOpenSetup(openSetup === db.id ? null : db.id)}>{openSetup === db.id ? <ChevronDown size={16} /> : <ChevronRight size={16} />}Setup</button>
        </div>
        {openSetup === db.id && <RigProfileCard key={db.id} slug={db.id} />}
      </li>;
    })}</ul>
  </section>;
}
