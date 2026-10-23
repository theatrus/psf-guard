import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { CollaborationVisit } from '../../api/collaborationTypes';
import EarlyBeta, { COLLABORATION_BETA } from './EarlyBeta';
import CollaborationLeaveDialog from './CollaborationLeaveDialog';

/** The plan's imported nightly assignments, to choose for activation, and
 *  Leave collaboration for a server that can change rig databases. */
export default function CollaborationActivation({ projectId, value, onChange, disabled, canLeave }: {
  projectId: string; value: CollaborationVisit[]; onChange: (value: CollaborationVisit[]) => void; disabled: boolean;
  /** An editor on a server with database management. */
  canLeave: boolean;
}) {
  const query = useQuery({ queryKey: ['collaborationActivation', projectId], queryFn: () => apiClient.getCollaborationActivation(projectId), retry: false });
  const [leaving, setLeaving] = useState(false);
  const dialog = leaving && <CollaborationLeaveDialog planId={projectId} onClose={() => setLeaving(false)} onLeft={() => onChange([])} />;
  if (query.isError) return <p role="alert">Collaboration assignments could not be loaded: {query.error.message}</p>;
  // The dialog keeps its place in the tree when the assignments go, so it
  // can still show what leaving did.
  if (!query.data?.imports.length) return <>{null}{dialog}</>;
  return <><fieldset disabled={disabled} className="collaboration-activation"><legend>Collaboration assignments <EarlyBeta note={COLLABORATION_BETA} /></legend>
    {query.data.imports.map(i => {
      const selected = value.find(v => v.import_id === i.import_id);
      return <div key={i.import_id}>
        <label><input type="checkbox" checked={!!selected} onChange={e => onChange(e.target.checked
          ? [...value.filter(v => !query.data.imports.some(other => other.rig_id === i.rig_id && other.import_id === v.import_id)), { import_id: i.import_id, source_digest: i.source_digest }]
          : value.filter(v => v.import_id !== i.import_id))} />{i.rig_name} · {i.night} · task {i.task_id} · revision {i.version}</label>
        {selected && <>
          <ul>{i.demands.map(d => <li key={`${d.panel_index}-${d.filter}`}>Panel {d.panel_index}: {d.requested_frames} × {d.exposure_ms / 1000} s {d.filter}</li>)}</ul>
        </>}
      </div>;
    })}
    {canLeave && <button type="button" className="header-button" onClick={() => setLeaving(true)}>Leave collaboration</button>}
  </fieldset>{dialog}</>;
}
