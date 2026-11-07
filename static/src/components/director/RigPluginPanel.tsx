import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Copy, KeyRound, Power, Settings, Trash2, X } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorEquipmentReport, DirectorPluginClient, DirectorPluginEquipment, DirectorPluginView, DirectorPluginWork, DirectorRigMode } from '../../api/directorTypes';
import { useAllDatabases } from '../../hooks/useDatabases';
import { openSettings } from '../../utils/settingsIntent';
import EarlyBeta from './EarlyBeta';

const MODE_LABEL = { target_scheduler: 'Target Scheduler', director: 'Director' } as const;

const WORK_NOTE: Record<DirectorPluginWork, string> = {
  not_allowed: 'Not allowed yet.',
  off: 'Off. The plugin gets no work.',
  ready: 'Allowed. The plugin can request work.',
  plugin_removed: 'Stopped. The plugin allowed to take work was removed.',
  equipment_changed: 'Stopped. The accepted equipment changed.',
  setup_changed: "Stopped. This rig's setup changed. Check the plans, then allow work again.",
};

const statusOf = (error: unknown) => isAxiosError(error) ? error.response?.status : undefined;
const problemOf = (error: unknown) => statusOf(error) === 409
  ? 'This rig changed. Check these steps and try again.'
  : isAxiosError(error) ? error.response?.data?.error || error.message
    : error instanceof Error ? error.message : 'The Director request failed';

function ago(ms: number): string {
  const minutes = Math.round((Date.now() - ms) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} min ago`;
  return new Date(ms).toLocaleString();
}

/** This server's address as the plugin needs it; a desktop window has none to offer. */
function serverAddress(): string | null {
  const { protocol, origin, pathname } = window.location;
  return /^https?:$/.test(protocol) ? `${origin}${pathname.replace(/\/$/, '')}` : null;
}

const needsHttps = (address: string | null) => !!address && address.startsWith('http:')
  && !/^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:|\/|$)/.test(address);

function filterList(equipment: DirectorPluginEquipment, names: Record<string, string>): string {
  return equipment.filters.map(filter => names[filter.id] ?? filter.id).join(', ') || 'None';
}

/** What a report would change, in the words the review shows. */
function reportChanges(report: DirectorEquipmentReport, view: DirectorPluginView): string {
  const accepted = view.configuration;
  if (!accepted) return 'Nothing accepted yet.';
  const changed = [
    report.configuration.camera_id !== accepted.camera_id && 'camera',
    filterList(report.configuration, report.filter_names) !== filterList(accepted, view.filter_names) && 'filters',
    report.configuration.id !== accepted.id && accepted.camera_id === report.configuration.camera_id && 'camera modes',
  ].filter(Boolean);
  return changed.length ? `Differs from the accepted equipment: ${changed.join(', ')}.` : 'Same as the accepted equipment.';
}

/** Pair, review and allow the N.I.N.A. Director plugin on one rig. */
export default function RigPluginPanel({ slug, mode, onRigChanged, onChangeMode }: {
  slug: string;
  /** Who picks the rig's work now. */
  mode: DirectorRigMode;
  /** Pairing and accepting equipment change the rig's profile. */
  onRigChanged: () => void;
  onChangeMode: () => void;
}) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const queryKey = ['directorPlugin', slug];
  const [code, setCode] = useState<{ token: string; expiresAt: number } | null>(null);
  const [copy, setCopy] = useState<'idle' | 'copied' | 'failed'>('idle');
  const loaded = useQuery({
    queryKey,
    queryFn: () => apiClient.getDirectorPlugin(slug),
    enabled: canWrite,
    // The plugin pairs and reports from N.I.N.A., so the page has to look.
    refetchInterval: code ? 3_000 : 15_000,
    // A save on another tab can stop work, so opening this tab always looks.
    staleTime: 0,
    refetchOnMount: 'always',
    refetchOnWindowFocus: false,
    retry: (count, error) => count < 5 && statusOf(error) === 503,
    retryDelay: 700,
  });
  const view = loaded.data;
  const act = useMutation({
    retry: false,
    mutationFn: (work: () => Promise<unknown>) => work(),
    onSettled: () => { void client.invalidateQueries({ queryKey }); },
  });
  const clientCount = view?.clients.length;
  const known = useRef(clientCount);
  useEffect(() => {
    if (clientCount === undefined) return;
    if (known.current !== undefined && known.current !== clientCount) {
      setCode(null);
      onRigChanged();
    }
    known.current = clientCount;
  }, [clientCount, onRigChanged]);
  const databases = useAllDatabases();
  const syncClients = databases.data?.find(db => db.id === slug)?.remote_image_upload?.clients?.length ?? 0;
  if (!canWrite) return <p className="director-muted">Sign in as an editor to manage the Director plugin.</p>;
  if (loaded.isPending) return <p role="status">Loading the Director plugin...</p>;
  if (!view) return <p className="director-error" role="alert">{problemOf(loaded.error)}</p>;

  const scope = { coordinator_instance_id: view.coordinator_instance_id, catalog_id: view.catalog_id };
  const address = serverAddress();
  const generate = () => act.mutate(async () => {
    const issued = await apiClient.issueDirectorPairingCode(view.rig_id, scope);
    setCopy('idle');
    setCode({ token: issued.pairing_token, expiresAt: issued.expires_at_ms });
  });
  const copyCode = async () => {
    if (!code) return;
    try { await navigator.clipboard.writeText(code.token); setCopy('copied'); } catch { setCopy('failed'); }
  };
  const remove = (entry: DirectorPluginClient) => {
    const holdsWork = view.grant?.open && view.grant.client_id === entry.client_id;
    const warning = holdsWork
      ? `Remove ${entry.client_name}? It has open work. The rig takes no new work until you close that work.`
      : `Remove ${entry.client_name}? The plugin must pair again to reach this rig.`;
    if (window.confirm(warning)) act.mutate(() => apiClient.removeDirectorClient(view.rig_id, entry.client_id));
  };
  const accept = (entry: DirectorPluginClient) => act.mutate(async () => {
    await apiClient.acceptDirectorEquipment(view.rig_id, entry.client_id, { ...scope, report_id: entry.report!.report_id, expected_revision: view.profile_revision });
    onRigChanged();
  });
  return <div className="rig-plugin">
    <div className="early-beta-heading"><h4>Plugins on this rig</h4><EarlyBeta /></div>
    <dl className="rig-plugin-paths">
      <div><dt>Picks the work</dt><dd>{MODE_LABEL[mode]} <button type="button" className="link-button" onClick={onChangeMode}>Change mode</button></dd></div>
      <div><dt>PSF Guard Director</dt><dd>{view.clients.length ? `${view.clients.length} paired` : 'Not paired'}</dd></div>
      <div><dt>PSF Guard Sync</dt><dd>{syncClients ? `${syncClients} paired` : 'Not paired'} <button type="button" className="link-button" onClick={() => openSettings('sync', { databaseId: slug })}><Settings size={14} />Open Sync</button></dd></div>
    </dl>
    <p className="director-muted">Both plugins can stay installed. N.I.N.A. runs the instruction in its sequence. Sync still moves images and catalog data in either mode.</p>

    {act.isError && <p className="director-error" role="alert">{problemOf(act.error)}</p>}

    <section aria-label="Pair" className="rig-plugin-step">
      <h5>1. Pair</h5>
      <ul className="rig-plugin-rows" aria-label="Paired Director plugins">
        {view.clients.map(entry => <li key={entry.client_id}>
          <span>{entry.client_name} <small className="director-muted">Paired {new Date(entry.created_at_ms).toLocaleDateString()}</small></span>
          <button type="button" disabled={act.isPending} aria-label={`Remove ${entry.client_name}`} onClick={() => remove(entry)}><Trash2 size={14} />Remove</button>
        </li>)}
      </ul>
      {view.clients.length === 0 && !code && <p className="director-muted">Install PSF Guard Director in N.I.N.A., then generate a code.</p>}
      <div className="director-actions">
        <button type="button" disabled={act.isPending} onClick={generate}><KeyRound size={16} />Generate Director pairing code</button>
      </div>
      {code && <div className="rig-plugin-code">
        <label className="rig-profile-field"><span>Pairing code</span>
          <span className="rig-profile-input"><input readOnly aria-label="Director pairing code" value={code.token} onFocus={event => event.target.select()} />
            <button type="button" onClick={() => void copyCode()}><Copy size={14} />{copy === 'copied' ? 'Copied' : 'Copy'}</button>
          </span>
        </label>
        {address && <p className="director-muted">Server URL: <code>{address}</code></p>}
        <p role="status">{copy === 'failed' ? 'Copy failed. Select and copy the code.' : `In N.I.N.A., open the Director plugin settings, enter the server URL and this code, then click Pair. One use, expires ${new Date(code.expiresAt).toLocaleTimeString()}. This page updates when the plugin pairs.`}</p>
        {needsHttps(address) && <p className="director-muted">This address uses HTTP. The plugin needs HTTPS unless you allow plain HTTP in its settings.</p>}
      </div>}
    </section>

    <section aria-label="Equipment" className="rig-plugin-step">
      <h5>2. Equipment</h5>
      {view.configuration
        ? <p>Accepted: {view.configuration.camera_id}. Filters: {filterList(view.configuration, view.filter_names)}.</p>
        : <p className="director-muted">No equipment accepted yet.</p>}
      {view.clients.length === 0 && <p className="director-muted">Pair the plugin first.</p>}
      {view.clients.map(entry => <EquipmentReview key={entry.client_id} entry={entry} view={view} busy={act.isPending} onAccept={() => accept(entry)} />)}
    </section>

    <AllowWork view={view} mode={mode} busy={act.isPending} onSave={change => act.mutate(() => apiClient.saveDirectorWorkPolicy(view.rig_id, change))} />

    <section aria-label="Work" className="rig-plugin-step">
      <h5>4. Work</h5>
      {view.last_contact_at_ms
        ? <p className="director-muted">Last status: {ago(view.last_contact_at_ms)}</p>
        : view.clients.length > 0 && <p className="director-muted">No status from the plugin yet.</p>}
      {!view.grant?.open && <p>No open work.</p>}
      {view.grant && !view.grant.open && view.grant.closed_at_ms && <p className="director-muted">Last work closed by an operator {ago(view.grant.closed_at_ms)}.</p>}
      {view.grant?.open && view.grant.plugin_paired && <p>{view.grant.started ? 'Running' : 'Not started'}. Issued {ago(view.grant.admitted_at_ms)}. The plugin ends this work when its session finishes.</p>}
      {view.grant?.open && !view.grant.plugin_paired && <>
        <p className="director-error" role="alert">The plugin with this work was removed, so this rig cannot take new work.</p>
        <p className="director-muted">Stop the Director Session in N.I.N.A. before closing. Attempts the plugin reported stay spent.</p>
        <div className="director-actions">
          <button type="button" disabled={act.isPending} onClick={() => {
            if (window.confirm('Close this work? Stop the Director Session in N.I.N.A. first. Attempts it reported stay spent, and this cannot be undone.')) {
              const id = view.grant!.allocation_id;
              act.mutate(() => apiClient.closeDirectorWork(view.rig_id, id, scope));
            }
          }}><X size={16} />Close work</button>
        </div>
      </>}
    </section>
  </div>;
}

function EquipmentReview({ entry, view, busy, onAccept }: { entry: DirectorPluginClient; view: DirectorPluginView; busy: boolean; onAccept: () => void }) {
  const report = entry.report;
  if (!report) return <p className="director-muted">{entry.client_name}: connect the equipment in N.I.N.A., then click Report equipment in the Director Session.</p>;
  if (entry.review === 'accepted') return <p className="director-muted">{entry.client_name}: report accepted.</p>;
  return <div className="rig-plugin-report">
    <p>{entry.client_name} reported {report.configuration.camera_id} {ago(report.received_at_ms)}. Filters: {filterList(report.configuration, report.filter_names)}.</p>
    <p className="director-muted">{reportChanges(report, view)}</p>
    {entry.review === 'ready' && <div className="director-actions"><button type="button" disabled={busy} onClick={onAccept}><Check size={16} />Accept equipment</button></div>}
    {entry.review === 'old' && <p className="director-muted">This report is more than 15 minutes old. Click Report equipment in the Director Session again.</p>}
    {entry.review === 'work_open' && <p className="director-muted">Equipment cannot change while work is open.</p>}
  </div>;
}

function AllowWork({ view, mode, busy, onSave }: {
  view: DirectorPluginView; mode: DirectorRigMode; busy: boolean;
  onSave: (change: { coordinator_instance_id: string; expected_revision: number; policy: NonNullable<DirectorPluginView['policy']> }) => void;
}) {
  // No edits (null) shows what is allowed now, or every plan the first time.
  const [picked, setPicked] = useState<string[] | null>(null);
  const [chosenClient, setChosenClient] = useState<string | null>(null);
  const policy = view.policy;
  const planIds = picked ?? (policy ? policy.project_ids.filter(id => view.plans.some(plan => plan.id === id)) : view.plans.map(plan => plan.id));
  const allowed = view.clients.find(entry => entry.client_id === chosenClient)
    ?? view.clients.find(entry => entry.client_id === policy?.client_id)
    ?? view.clients[view.clients.length - 1];
  const save = (enabled: boolean) => {
    if (!allowed || !view.configuration) return;
    const expected = policy?.revision ?? 0;
    onSave({
      coordinator_instance_id: view.coordinator_instance_id,
      expected_revision: expected,
      policy: {
        rig_id: view.rig_id, catalog_id: view.catalog_id, client_id: allowed.client_id, profile_id: allowed.profile_id,
        profile_revision: view.profile_revision, configuration_id: view.configuration.id,
        project_ids: planIds, enabled, revision: expected + 1,
      },
    });
    setPicked(null);
  };
  const blocked = !allowed ? 'Pair the plugin first.' : !view.configuration ? 'Accept the equipment first.'
    : view.plans.length === 0 ? 'Activate a plan on this rig first.' : null;
  return <section aria-label="Allow work" className="rig-plugin-step">
    <h5>3. Allow work</h5>
    <p data-testid="rig-plugin-work" data-state={view.work}>{WORK_NOTE[view.work]}</p>
    {blocked && <p className="director-muted">{blocked}</p>}
    {!blocked && <>
      {view.clients.length > 1 && <label className="rig-profile-field"><span>Plugin</span>
        <span className="rig-profile-input"><select aria-label="Plugin allowed to take work" value={allowed!.client_id} onChange={event => setChosenClient(event.target.value)}>
          {view.clients.map(entry => <option key={entry.client_id} value={entry.client_id}>{entry.client_name} ({new Date(entry.created_at_ms).toLocaleDateString()})</option>)}
        </select></span>
      </label>}
      <fieldset className="rig-plugin-plans"><legend>Plans the plugin may run</legend>
        {view.plans.map(plan => <label key={plan.id}>
          <input type="checkbox" checked={planIds.includes(plan.id)} onChange={event => setPicked(event.target.checked ? [...planIds, plan.id] : planIds.filter(id => id !== plan.id))} />
          {plan.name}
        </label>)}
      </fieldset>
      <div className="director-actions">
        <button type="button" disabled={busy || planIds.length === 0} onClick={() => save(true)}><Check size={16} />{view.work === 'ready' ? 'Save plans' : policy ? 'Allow work again' : 'Allow work'}</button>
        {view.work === 'ready' && <button type="button" disabled={busy} onClick={() => save(false)}><Power size={16} />Turn off</button>}
      </div>
      {view.work === 'ready' && mode === 'target_scheduler' && <p className="director-muted">Mode is Target Scheduler. The plugin still gets work when the N.I.N.A. sequence runs Director.</p>}
    </>}
  </section>;
}
