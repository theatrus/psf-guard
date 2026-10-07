import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { CollaborationVisit } from '../../api/collaborationTypes';

export default function CollaborationActivation({ projectId, value, onChange, disabled }: {
  projectId: string; value: CollaborationVisit[]; onChange: (value: CollaborationVisit[]) => void; disabled: boolean;
}) {
  const query = useQuery({ queryKey: ['collaborationActivation', projectId], queryFn: () => apiClient.getCollaborationActivation(projectId), retry: false });
  if (query.isError) return <p role="alert">Collaboration assignments could not be loaded: {query.error.message}</p>;
  if (!query.data?.imports.length) return null;
  return <fieldset disabled={disabled} className="collaboration-activation"><legend>Collaboration assignments</legend>
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
  </fieldset>;
}
