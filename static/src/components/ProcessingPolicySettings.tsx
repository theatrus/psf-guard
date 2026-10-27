import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { DatabasePolicy, PolicyOverrides, PolicyValues, ProcessingPolicies } from '../api/processingPolicies';
import { useAccess } from '../auth/access';
import UnsavedMark from './UnsavedMark';
import QualityBackfillControls from './QualityBackfillControls';

const FIELDS = {
  analyze_new_frames: { label: 'Analyze new frames as they arrive', help: 'Analyze frames received by sync, uploads or automatic import. Turning this on also queues frames already waiting for analysis.' },
  scan_calibrated_copies: { label: 'Measure quality on the calibrated copy', help: 'Use a light’s calibrated copy when available. Registered copies are never measured. Updated results appear the next time quality analysis runs.' },
  pair_calibrated_copies: { label: 'Pair calibrated and registered copies with their lights', help: 'Record processed copies with their original light instead of importing them as separate frames. Changes apply to future imports and pairing scans.' },
};
type Key = keyof PolicyValues;

export default function ProcessingPolicySettings({ kind, canManage }: { kind: 'quality' | 'pairing'; canManage: boolean }) {
  const query = useQuery({ queryKey: ['processing-policies'], queryFn: apiClient.getProcessingPolicies, retry: false });
  const keys: Key[] = kind === 'quality' ? ['analyze_new_frames', 'scan_calibrated_copies'] : ['pair_calibrated_copies'];
  const title = kind === 'quality' ? 'Quality analysis' : 'Calibrated copies';
  if (query.isPending) return <section className="settings-section"><h3>{title}</h3><p role="status">Loading processing settings…</p></section>;
  if (query.isError) return <section className="settings-section"><h3>{title}</h3><p role="alert">Could not load processing settings. {query.error.message}</p><button type="button" onClick={() => void query.refetch()}>Retry</button></section>;
  return <section className="settings-section" aria-label={title}>
    <h3>{title}</h3><p>Shared defaults apply to databases that have no override. Existing database choices are preserved as overrides. Save a database override to change only that database.</p>
    <PolicyEditor keys={keys} data={query.data} canManage={canManage} />
    <h4>Database overrides</h4>
    {query.data.databases.length === 0 && <p>Add a database to configure overrides.</p>}
    {query.data.databases.map(entry => <details key={entry.id} className="calibration-settings-group">
      <summary>{entry.name} · {keys.some(key => entry.overrides[key] !== null) ? 'Has overrides' : 'Uses global settings'}</summary>
      <PolicyEditor keys={keys} data={query.data} database={entry} canManage={canManage} />
      {kind === 'quality' && <QualityBackfillControls dbId={entry.id} canManage={canManage} showArrivalSetting={false} />}
    </details>)}
  </section>;
}
function PolicyEditor({ keys, data, database, canManage }: { keys: Key[]; data: ProcessingPolicies; database?: DatabasePolicy; canManage: boolean }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const [edits, setEdits] = useState<Partial<PolicyOverrides>>({});
  const saved = database?.overrides ?? data.defaults;
  const dirty = keys.some(key => key in edits && edits[key] !== saved[key]);
  const label = database ? `${database.name} processing` : 'processing defaults';
  const save = useMutation({
    mutationFn: () => apiClient.updateProcessingPolicies({ database_id: database?.id, values: Object.fromEntries(keys.filter(key => key in edits && edits[key] !== saved[key]).map(key => [key, edits[key]])) }),
    onSuccess: updated => {
      client.setQueryData(['processing-policies'], updated);
      for (const entry of updated.databases) {
        void client.invalidateQueries({ queryKey: ['db', entry.id, 'quality-backfill'] });
        void client.invalidateQueries({ queryKey: ['db', entry.id, 'calibrated-copies'] });
      }
      setEdits({});
    },
  });
  const disabled = !canWrite || !canManage || save.isPending;
  return <div className="processing-policy-editor calibration-matching-group" role="region" aria-label={database ? `${database.name} overrides` : 'Shared defaults'}>
    <UnsavedMark label={label} dirty={dirty} />
    {!database && <h4>Shared defaults</h4>}
    {keys.map(key => {
      const own = key in edits ? edits[key] : saved[key];
      const value = own ?? data.defaults[key];
      return <div key={key} className="review-preference calibration-rule-row">
        <span>{FIELDS[key].label}<small>{FIELDS[key].help}</small></span>
        <div className="calibration-rule-setting">
          {own !== null ? <label><input type="checkbox" aria-label={FIELDS[key].label} checked={!!value} disabled={disabled} onChange={event => setEdits(previous => ({ ...previous, [key]: event.target.checked }))} /> {value ? 'On' : 'Off'}</label>
            : <p className="calibration-rule-inherited">{value ? 'On' : 'Off'}<small> · from shared defaults</small></p>}
          {database && <button type="button" className="calibration-rule-own" aria-pressed={own !== null} aria-label={`${FIELDS[key].label}: override global setting`} disabled={disabled}
            onClick={() => setEdits(previous => ({ ...previous, [key]: own === null ? data.defaults[key] : null }))}>Override global setting</button>}
        </div>
      </div>;
    })}
    {!canManage && <p className="muted">Changing processing settings requires database management on this server.</p>}
    {save.isError && <p role="alert">{save.error.message}</p>}
    <button type="button" className="save-button" disabled={disabled || !dirty} onClick={() => save.mutate()}>{save.isPending ? 'Saving…' : database ? 'Save database overrides' : 'Save shared defaults'}</button>
  </div>;
}
