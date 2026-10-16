import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, Eye, Pause, Play, RotateCw, Upload, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { ContributionSelection, CollaborationWorkInput } from '../../api/collaborationTypes';
import type { TransferAction, TransferJob, TransferManifest } from '../../api/transferTypes';
import { TRANSFER_QUERY_KEY } from '../../api/transferTypes';
import './CollaborationTransfers.css';

const transferLabel = (state: TransferJob['state']) => ({ preparing: 'Preparing', review: 'Awaiting review', queued: 'Queued', uploading: 'Uploading', paused: 'Paused', retry: 'Retry scheduled', needs_review: 'Needs attention', complete: 'Delivered', cancelled: 'Cancelled' })[state];
const transferBytes = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MiB`;

export function TransferControls({ job, busy, run }: { job: TransferJob; busy: boolean; run: (action: TransferAction) => void }) {
  const active = ['preparing', 'queued', 'uploading', 'retry'].includes(job.state);
  return <div className="director-actions">
    {job.state === 'review' && <button type="button" disabled={busy} onClick={() => run('approve')}><Check size={16} />Upload reviewed package</button>}
    {active && <button type="button" title="Pause transfer" aria-label="Pause transfer" disabled={busy} onClick={() => run('pause')}><Pause size={16} /></button>}
    {['paused', 'retry'].includes(job.state) && <button type="button" title="Resume transfer" aria-label="Resume transfer" disabled={busy} onClick={() => run('resume')}><Play size={16} /></button>}
    {['needs_review', 'cancelled'].includes(job.state) && <button type="button" title="Prepare and review again" aria-label="Prepare and review again" disabled={busy} onClick={() => run('retry')}><RotateCw size={16} /></button>}
    {!['complete', 'cancelled'].includes(job.state) && <button type="button" title="Cancel pending transfer" aria-label="Cancel pending transfer" disabled={busy} onClick={() => run('cancel')}><X size={16} /></button>}
  </div>;
}

export default function CollaborationTransfers({ connection, selection, canWrite, active, selectionReady }: { connection: string; selection: ContributionSelection; canWrite: boolean; active: boolean; selectionReady: boolean }) {
  const [outputKey, setOutput] = useState('');
  const [confirmedFor, setConfirmed] = useState<string | null>(null);
  const confirmationKey = JSON.stringify(selection) + outputKey;
  const confirmed = confirmedFor === confirmationKey;
  const [manifest, setManifest] = useState<{ id: string; revision: number; value: TransferManifest } | null>(null);
  const client = useQueryClient();
  const outputs = useQuery({ queryKey: ['transfer-outputs', connection, selection], enabled: active && selectionReady && canWrite,
    queryFn: () => apiClient.collaborationWork(connection, { operation: 'transfer_outputs', selection }), retry: false });
  const activity = useQuery({ queryKey: TRANSFER_QUERY_KEY, enabled: active, queryFn: apiClient.getTransfers,
    refetchInterval: (q) => q.state.data?.jobs.some(j => ['preparing', 'queued', 'uploading'].includes(j.state)) ? 2000 : 10_000 });
  const mutation = useMutation({ retry: false, mutationFn: (input: CollaborationWorkInput) => apiClient.collaborationWork(connection, input),
    onSuccess: (result, input) => {
      if (input.operation === 'transfer_manifest' && result.transfer_manifest) setManifest({ id: input.id, revision: input.revision, value: result.transfer_manifest });
      else void client.invalidateQueries({ queryKey: TRANSFER_QUERY_KEY });
    } });
  const output = outputs.data?.outputs?.find(o => `${o.job_id}:${o.group_index}:${o.revision}` === outputKey);
  const jobs = activity.data?.jobs.filter(j => j.connection_id === connection) ?? [];
  const policy = activity.data?.policies?.find(p => p.connection_id === connection);
  const policyBusy = !canWrite || mutation.isPending || !activity.data?.policies || activity.isError;
  const validOutput = !outputKey || !!output;
  return <section className="collaboration-transfers" aria-label="Contribution files">
    <h4>Contribution files</h4>
    <div className="rig-profile-grid">
      <label><input type="checkbox" disabled={policyBusy} checked={policy?.automatic ?? false} onChange={e => mutation.mutate({ operation: 'transfer_policy', automatic: e.target.checked, mode: policy?.mode ?? 'raw' })} />Upload accepted collaboration files automatically</label>
      <label className="rig-profile-field"><span>Automatic files</span><select disabled={policyBusy} value={policy?.mode ?? 'raw'} onChange={e => mutation.mutate({ operation: 'transfer_policy', automatic: policy?.automatic ?? false, mode: e.target.value })}><option value="raw">Raw lights and matched calibration</option><option value="psf_guard">PSF Guard stacks</option></select></label>
    </div>
    <div className="rig-profile-grid"><label className="rig-profile-field"><span>Files to send</span>
      <select value={outputKey} disabled={!canWrite || mutation.isPending} onChange={e => { setOutput(e.target.value); setConfirmed(null); }}>
        <option value="">Raw lights and matched calibration</option>
        {outputs.data?.outputs?.filter(o => o.wbpp || o.matches_selection).map(o => <option key={`${o.job_id}:${o.group_index}`} value={`${o.job_id}:${o.group_index}:${o.revision}`}>{o.label}</option>)}
      </select></label></div>
    {output?.wbpp && <label><input type="checkbox" disabled={!canWrite} checked={confirmed} onChange={e => setConfirmed(e.target.checked ? confirmationKey : null)} />I confirm this WBPP stack contains the selected images</label>}
    <button type="button" disabled={!canWrite || mutation.isPending || !selectionReady || !validOutput || (!!output?.wbpp && !confirmed)}
      onClick={() => mutation.mutate({ operation: 'prepare_transfer', request: { selection, ...(output ? { output: { job_id: output.job_id, group_index: output.group_index, revision: output.revision } } : {}), confirm_wbpp_membership: confirmed } })}>
      <Upload size={16} />Prepare file package
    </button>
    {[outputs.error, activity.error, mutation.error].filter(Boolean).map((e, i) => <p key={i} role="alert">{e!.message}</p>)}
    {jobs.map(job => <section key={job.id} aria-label={`Transfer ${job.label}`}>
      <h5>{job.label}</h5><p role="status">{transferLabel(job.state)} · {job.completed_files}/{job.total_files} files · {transferBytes(job.completed_bytes + job.sent_bytes)} / {transferBytes(job.total_bytes)}</p>
      {job.current_file && <p>{job.current_file}</p>}
      {job.error && <p role="alert">{job.error}</p>}
      {job.warnings?.map(w => <p key={w}>{w}</p>)}
      {job.state === 'review' && <button type="button" disabled={!canWrite || mutation.isPending} onClick={() => mutation.mutate({ operation: 'transfer_manifest', id: job.id, revision: job.revision })}><Eye size={16} />Inspect package</button>}
      {manifest?.id === job.id && manifest.revision === job.revision && <div>
        <p>{manifest.value.files.length} file{manifest.value.files.length === 1 ? '' : 's'} · Panel {manifest.value.panel}{manifest.value.night ? ` · ${manifest.value.night}` : ''}</p>
        <div className="collaboration-transfer-files"><table aria-label="Package files"><thead><tr><th>File</th><th>Type</th><th>Size</th></tr></thead><tbody>
          {manifest.value.files.map((file, i) => <tr key={`${file.kind}:${file.source.path}:${i}`}><td title={file.source.path}>{file.source.path.split(/[\\/]/).at(-1)}</td><td>{file.kind}</td><td>{transferBytes(file.source.bytes)}</td></tr>)}
        </tbody></table></div>
        <details><summary>Technical manifest</summary><pre>{JSON.stringify(manifest.value, null, 2)}</pre></details>
      </div>}
      {canWrite && <TransferControls job={job} busy={mutation.isPending} run={action => mutation.mutate({ operation: 'transfer_control', id: job.id, revision: job.revision, action })} />}
    </section>)}
  </section>;
}
