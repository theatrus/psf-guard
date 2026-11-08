import { useEffect, useId, useState, type FormEvent, type ReactNode } from 'react';
import { type QueryClient, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError, type AxiosError } from 'axios';
import { Camera, Check, CloudSun, Download, Link2, MapPin, Plug, RefreshCw, SlidersHorizontal } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorRigProfileView, DirectorRigSite } from '../../api/directorTypes';
import type { ObservingSettings, SchedulingOverrides } from '../../api/directorPreferences';
import HorizonEditor from './HorizonEditor';
import CollaborationConnections from './CollaborationConnections';
import RigSchedulingApply from './RigSchedulingApply';
import SchedulingFields from './SchedulingFields';
import { compactOverrides, inheritedLimits, sameOverrides } from './schedulingModel';
import EarlyBeta from './EarlyBeta';
import RigPluginPanel from './RigPluginPanel';
import RigWeatherPanel from './RigWeather';
import WorkspaceTabs from './WorkspaceTabs';
import { workspacePanel } from './workspacePanel';
import { applyDefaults, describeSource, editFromForm, fieldOfView, filterNames, formFromProfile, formatFieldOfView, opticsFromForm, type RigProfileForm } from './rigProfileForm';
import { useUnsavedChanges } from '../../hooks/useUnsavedChanges';

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Rig profile request failed';
/** The profile was saved, but its planning site and limits were not. */
class SettingsNotSaved extends Error {}
// The HTTP error behind a failure, through the errors that wrap it.
const httpErrorOf = (error: unknown): AxiosError | null => isAxiosError(error) ? error
  : error instanceof Error ? httpErrorOf(error.cause) : null;

const TABS = [{ id: 'optics', label: 'Optics', icon: <Camera size={16} /> }, { id: 'site', label: 'Site', icon: <MapPin size={16} /> }, { id: 'limits', label: 'Limits and delivery', icon: <SlidersHorizontal size={16} /> }, { id: 'weather', label: 'Weather', icon: <CloudSun size={16} /> }, { id: 'director', label: 'Plugins', icon: <Plug size={16} /> }, { id: 'collaboration', label: 'Collaboration', icon: <Link2 size={16} /> }] as const;
type Tab = (typeof TABS)[number]['id'];
/** A Rig setup tab. `director` is the Plugins tab: its id predates the Sync plugin moving there. */
export type RigSetupTab = Tab;

function Field({ id, label, value, onChange, disabled, unit, step = 'any' }: {
  id: string; label: string; value: string; onChange: (value: string) => void; disabled: boolean; unit?: string; step?: string;
}) {
  return <label className="rig-profile-field" htmlFor={id}>
    <span>{label}</span>
    <span className="rig-profile-input"><input id={id} aria-label={label} type="number" inputMode="decimal" step={step} value={value} disabled={disabled} onChange={event => onChange(event.target.value)} />{unit && <small>{unit}</small>}</span>
  </label>;
}

/** Optics, site and limits for the rig behind one database. */

const MODE_LABEL = { target_scheduler: 'Target Scheduler', director: 'Director' } as const;

const MODE_HELP = {
  target_scheduler: "Target Scheduler picks the rig's targets. Use its instruction in the rig's N.I.N.A. sequence.",
  director: "The Director plugin picks the rig's targets from the plans activated on it. Use the Director instruction in the rig's N.I.N.A. sequence.",
} as const;
/** The HTTP status behind a request error, which the client may wrap. */
function statusOf(error: unknown): number | undefined {
  if (isAxiosError(error)) return error.response?.status;
  return error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
}

/** The rig's profile. A database added since the plans were last listed has
 *  no rig yet; listing the plans creates it, so list them once and ask again. */
async function profileOrRig(slug: string, client: QueryClient): Promise<DirectorRigProfileView> {
  try {
    return await apiClient.getDirectorRigProfile(slug);
  } catch (error) {
    if (statusOf(error) !== 404) throw error;
    try {
      await apiClient.getDirectorPlans();
    } catch {
      throw error;
    }
    void client.invalidateQueries({ queryKey: ['directorPlans'] });
    return apiClient.getDirectorRigProfile(slug);
  }
}

export default function RigProfileCard({ slug, syncPlugin, initialTab }: {
  slug: string;
  /** The Sync plugin's section, shown on the Plugins tab. Settings owns its form. */
  syncPlugin?: ReactNode;
  initialTab?: Tab;
}) {
  const id = useId();
  const [tab, setTab] = useState<Tab>(initialTab ?? 'optics');
  const [collaborationVisited, setCollaborationVisited] = useState(false);
  const [vertical, setVertical] = useState(() => window.matchMedia?.('(min-width: 800px)').matches ?? false);
  useEffect(() => {
    const media = window.matchMedia?.('(min-width: 800px)');
    if (!media) return;
    const update = () => setVertical(media.matches);
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const queryKey = ['directorRigProfile', slug];
  const loaded = useQuery({ queryKey, queryFn: () => profileOrRig(slug, client), refetchOnWindowFocus: false,
    // Director admits one metadata request at a time; a 503 means wait, not fail.
    retry: (count, error) => count < 5 && statusOf(error) === 503, retryDelay: 700 });
  const peers = useQuery({ queryKey: ['peers'], queryFn: apiClient.getPeers, retry: false, refetchOnWindowFocus: false });
  // Where the rig stands for planning: the site it names, and what that
  // gives it. The rig's own location and horizon still win.
  const rigId = loaded.data?.rig.id;
  const sites = useQuery({ queryKey: ['observingDefaults'], queryFn: apiClient.getObservingDefaults, refetchOnWindowFocus: false });
  const rigSettings = useQuery({ queryKey: ['observingSettings', 'rig', rigId], queryFn: () => apiClient.getObservingSettings('rig', rigId!), enabled: !!rigId, refetchOnWindowFocus: false });
  const summaries = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, refetchOnWindowFocus: false });
  const placed = summaries.data?.find(entry => entry.catalog_slug === slug)?.site;
  const [form, setForm] = useState<RigProfileForm | null>(null);
  // Edits to the planning site and the rig's Target Scheduler limits, saved
  // with its observing settings, and the copy of those settings they started
  // from. Without edits (null), the fields show the saved values, so a newer
  // copy reaches them. Save sends the edits with the revision they started
  // from, so the server refuses them if the settings changed since.
  const [settingsEdit, setSettingsEdit] = useState<{ siteId: string | null; scheduling: SchedulingOverrides; base: ObservingSettings } | null>(null);
  const siteId = settingsEdit ? settingsEdit.siteId : rigSettings.data?.site_id ?? null;
  const scheduling = settingsEdit ? settingsEdit.scheduling : rigSettings.data?.scheduling ?? {};
  // Edits put back to the values they started from are no edits, while
  // those values are still the saved ones.
  const editSettings = (patch: { siteId?: string | null; scheduling?: SchedulingOverrides }) => setSettingsEdit(current => {
    const base = current?.base ?? rigSettings.data;
    if (!base) return current;
    const next = { siteId: current ? current.siteId : base.site_id, scheduling: current ? current.scheduling : base.scheduling ?? {}, ...patch, base };
    const unchanged = next.siteId === base.site_id && sameOverrides(next.scheduling, base.scheduling ?? {});
    return unchanged && base.revision === rigSettings.data?.revision ? null : next;
  });
  const [problem, setProblem] = useState('');
  const [notice, setNotice] = useState('');
  useEffect(() => { if (loaded.data) setForm(formFromProfile(loaded.data.profile)); }, [loaded.data]);
  const schedulingChanged = !!rigSettings.data && !sameOverrides(scheduling, rigSettings.data.scheduling ?? {});
  // What the rig inherits: every plan's limits, then its site's.
  const globalSettings = useQuery({ queryKey: ['observingSettings', 'global', sites.data?.global_id], queryFn: () => apiClient.getObservingSettings('global', sites.data!.global_id), enabled: !!sites.data, refetchOnWindowFocus: false });
  const siteSettings = useQuery({ queryKey: ['observingSettings', 'site', siteId], queryFn: () => apiClient.getObservingSettings('site', siteId!), enabled: !!siteId, refetchOnWindowFocus: false });
  const parentLimits = inheritedLimits([
    { overrides: globalSettings.data?.scheduling, from: 'from every plan' },
    siteId ? { overrides: siteSettings.data?.scheduling, from: 'from the site' } : null,
  ]);
  const save = useMutation({
    retry: false,
    mutationFn: async (view: DirectorRigProfileView) => {
      if (!form) throw new Error('Nothing to save');
      const edit = editFromForm(form, view.profile);
      if (typeof edit === 'string') throw new Error(edit);
      const saved = await apiClient.saveDirectorRigProfile(slug, edit);
      // Cached at once, so a failed site step below leaves the next Save
      // naming this revision rather than conflicting with itself. Keep the
      // header defaults from the last load; a save does not reread frames.
      client.setQueryData<DirectorRigProfileView>(queryKey, current => current ? { ...saved, defaults: current.defaults } : saved);
      // The planning site and the Target Scheduler limits live with the
      // rig's observing settings.
      if (rigSettings.data && (siteId !== rigSettings.data.site_id || schedulingChanged)) {
        try {
          const settings = await apiClient.saveObservingSettings({ ...(settingsEdit?.base ?? rigSettings.data), site_id: siteId, scheduling: compactOverrides(scheduling) });
          client.setQueryData(['observingSettings', 'rig', settings.scope_id], settings);
          void client.invalidateQueries({ queryKey: ['observingEffective'] });
        } catch (error) {
          throw new SettingsNotSaved(`Rig profile saved, but site and limits could not be saved: ${message(error)}`, { cause: error });
        }
      }
      return saved;
    },
    onSuccess: saved => {
      setNotice(`Saved rig profile revision ${saved.profile.revision}.`);
      // The site and limits are saved too, or matched what was saved.
      setSettingsEdit(null);
    },
    // Either step may have changed what planning reads for the rig.
    onSettled: () => { void client.invalidateQueries({ queryKey: ['directorRigProfiles'] }); void client.invalidateQueries({ queryKey: ['rigScheduling'] }); },
  });
  // Derived from the error itself, so the conflict notice and the generic one
  // can never both render for the same failed save. A refused site and
  // limits step comes wrapped in the message that the profile was saved.
  const httpError = httpErrorOf(save.error);
  const stale = httpError?.response?.status === 409;
  // Reload drops site and limit edits only when the settings under them
  // changed, as it drops the profile's fields only when the profile did.
  const reload = () => {
    setNotice(''); setProblem(''); save.reset(); void loaded.refetch();
    if (rigId) void rigSettings.refetch().then(result => {
      const latest = result.data;
      if (latest) setSettingsEdit(current => current && current.base.revision !== latest.revision ? null : current);
    });
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!loaded.data || !form || !canWrite || save.isPending) return;
    setNotice(''); setProblem('');
    const edit = editFromForm(form, loaded.data.profile);
    if (typeof edit === 'string') { setProblem(edit); return; }
    save.mutate(loaded.data);
  };
  const update = (patch: Partial<RigProfileForm>) => setForm(current => current ? { ...current, ...patch } : current);
  const data = loaded.data;
  // Edits Save has not sent: the form against the profile it loaded, the
  // planning site and the Target Scheduler limits. Settings asks before
  // closing over them.
  const siteChanged = !!rigSettings.data && siteId !== rigSettings.data.site_id;
  // The filter names the database's templates and frames use.
  const suggestedFilters = data?.defaults.filters ?? [];
  const unsaved = !!data && !!form && canWrite
    && (JSON.stringify(form) !== JSON.stringify(formFromProfile(data.profile)) || siteChanged || schedulingChanged);
  useUnsavedChanges(`${data?.rig.name ?? slug} setup`, unsaved);
  const disabled = !canWrite || save.isPending || stale;
  const optics = form ? opticsFromForm(form) : null;
  const preview = optics ? fieldOfView(optics) : null;
  return <section className="rig-profile" aria-label="Rig profile">
    <div className="director-toolbar rig-profile-heading"><h4>Setup</h4>
      <button type="button" aria-label="Reload rig profile" title="Reload rig profile" disabled={loaded.isFetching || save.isPending} onClick={reload}><RefreshCw size={16} /></button>
    </div>
    {loaded.isPending && <p role="status">Loading rig profile...</p>}
    {/* A database with no rig yet still pairs the Sync plugin; its record line
        already says how to add the rig, so the not-found error stays out. */}
    {loaded.isError && !(syncPlugin && statusOf(loaded.error) === 404)
      && <p className="director-error" role="alert">{message(loaded.error)}</p>}
    {loaded.isError && syncPlugin}
    {notice && <p role="status">{notice}</p>}
    {stale && <p className="director-error" role="alert">{save.error instanceof SettingsNotSaved
      ? 'The rig profile was saved. Its planning site and Target Scheduler limits changed since you loaded them. '
      : 'This rig changed since you loaded it. '}<button type="button" onClick={reload}>Reload</button></p>}
    {data && form && <div className="rig-setup-layout">
    <WorkspaceTabs id={id} className="rig-setup-nav" orientation={vertical ? 'vertical' : 'horizontal'} label="Rig setup sections" tabs={TABS} value={tab} onChange={next => { setTab(next); if (next === 'collaboration') setCollaborationVisited(true); }} />
    <div className="rig-setup-content">
    <form onSubmit={submit} className="rig-profile-form" hidden={tab === 'collaboration' || tab === 'weather' || tab === 'director'}>
      <div {...workspacePanel(id, 'optics', tab)}>
      <fieldset disabled={disabled}>
        <legend>Mode</legend>
        <div className="rig-profile-mode" role="radiogroup" aria-label="Who runs this rig">
          {(['target_scheduler', 'director'] as const).map(mode => <label key={mode}>
            <input type="radio" name={`${slug}-mode`} value={mode} checked={form.mode === mode} onChange={() => update({ mode })} />
            {MODE_LABEL[mode]}
          </label>)}
        </div>
        <p className="director-muted">{form.mode === ''
          ? `Not set. PSF Guard plans this rig as ${MODE_LABEL[data.mode.runtime]}, from what the rig has reported.`
          : MODE_HELP[form.mode]}</p>
        {(form.mode || data.mode.runtime) === 'director' && <ul className="rig-profile-needs" aria-label="What Director needs">
          <li data-done={data.mode.paired}>{data.mode.paired ? 'The N.I.N.A. Director plugin is paired with this rig.' : 'Pair the N.I.N.A. Director plugin with this rig.'}</li>
          <li data-done={data.mode.equipment}>{data.mode.equipment ? 'The plugin reported the rig\'s equipment.' : 'Connect equipment in N.I.N.A. and run Director Session.'}</li>
        </ul>}
        <p className="director-muted rig-profile-source">Changing the mode keeps the Director and Sync pairings.
          <button type="button" className="link-button" onClick={() => setTab('director')}><Plug size={14} />Set up Director</button>
        </p>
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Optics</legend>
        <p className="director-muted rig-profile-source">{describeSource(form.opticsSource ?? data.profile.optics?.source, data.profile.optics?.reported_at_ms)}
          {canWrite && data.defaults.optics && <button type="button" className="link-button" onClick={() => { update(applyDefaults(form, { ...data.defaults, site: null })); setNotice(''); }}><Download size={14} />Use frame headers ({data.defaults.optics.source.kind === 'frame_headers' ? data.defaults.optics.source.file_name : 'newest frame'})</button>}
        </p>
        <div className="rig-profile-grid">
          <Field id={`${slug}-sensor-width`} label="Sensor width" unit="px" step="1" value={form.sensorWidth} disabled={disabled} onChange={sensorWidth => update({ sensorWidth, opticsSource: { kind: 'manual' } })} />
          <Field id={`${slug}-sensor-height`} label="Sensor height" unit="px" step="1" value={form.sensorHeight} disabled={disabled} onChange={sensorHeight => update({ sensorHeight, opticsSource: { kind: 'manual' } })} />
          <Field id={`${slug}-pixel-size`} label="Pixel size" unit="µm" value={form.pixelSize} disabled={disabled} onChange={pixelSize => update({ pixelSize, opticsSource: { kind: 'manual' } })} />
          <Field id={`${slug}-focal-length`} label="Focal length" unit="mm" value={form.focalLength} disabled={disabled} onChange={focalLength => update({ focalLength, opticsSource: { kind: 'manual' } })} />
          <Field id={`${slug}-aperture`} label="Aperture" unit="mm" value={form.aperture} disabled={disabled} onChange={aperture => update({ aperture, opticsSource: { kind: 'manual' } })} />
          <label className="rig-profile-field rig-profile-rotation" htmlFor={`${slug}-rotation`}><span>Camera angle</span>
            <span className="rig-profile-input">
              <select id={`${slug}-rotation`} aria-label="Camera angle" value={form.rotationMode} onChange={event => update({ rotationMode: event.target.value as RigProfileForm['rotationMode'], opticsSource: { kind: 'manual' } })}>
                <option value="rotator">Rotator</option>
                <option value="manual">Turned by hand</option>
                <option value="fixed">Fixed</option>
              </select>
              {form.rotationMode !== 'rotator' && <input aria-label="Camera angle degrees" type="number" inputMode="decimal" step="any" value={form.rotationAngle} onChange={event => update({ rotationAngle: event.target.value, opticsSource: { kind: 'manual' } })} />}
              {form.rotationMode !== 'rotator' && <small>°</small>}
            </span>
          </label>
        </div>
        <p className="director-muted" data-testid="rig-profile-fov">{preview ? `Field ${formatFieldOfView(preview)}` : 'Enter the sensor size, pixel size and focal length to see the field of view.'}</p>
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Filters</legend>
        <p className="director-muted">{data.profile.configuration
          ? `The Director plugin reported this rig's filters: ${data.filter_list?.join(', ') || 'none'}. Planning checks templates against those, not the names below.`
          : "Planning warns when a template uses a filter that isn't on this list. Leave it empty to skip the check."}</p>
        <label className="rig-profile-field rig-profile-filters" htmlFor={`${slug}-filters`}><span>Filter names, separated by commas</span>
          <span className="rig-profile-input"><input id={`${slug}-filters`} aria-label="Filter names" type="text" placeholder="L, R, G, B, Ha, OIII, SII" value={form.filters} disabled={disabled} onChange={event => update({ filters: event.target.value })} /></span>
        </label>
        {canWrite && suggestedFilters.length > 0 && filterNames(form.filters).join(', ') !== suggestedFilters.join(', ') && <p className="director-muted rig-profile-source">
          <button type="button" className="link-button" onClick={() => update({ filters: suggestedFilters.join(', ') })}><Download size={14} />Use this database's filters ({suggestedFilters.join(', ')})</button>
        </p>}
      </fieldset>
      </div>
      <div {...workspacePanel(id, 'site', tab)}>
      <fieldset disabled={disabled}>
        <legend>Site</legend>
        <label className="rig-profile-field" htmlFor={`${slug}-planning-site`}><span>Planning site</span>
          <span className="rig-profile-input">
            <select id={`${slug}-planning-site`} aria-label="Planning site" value={siteId ?? ''} disabled={!rigSettings.data} onChange={event => editSettings({ siteId: event.target.value || null })}>
              <option value="">None</option>
              {sites.data?.sites.map(site => <option key={site.id} value={site.id}>{site.name}</option>)}
            </select>
          </span>
        </label>
        <p className="director-muted" data-testid="rig-site-origin">{describePlacement(placed)}</p>
        <p className="director-muted">Blank fields use the site's location and horizon.</p>
        <p className="director-muted rig-profile-source">{describeSource(form.siteSource ?? data.profile.site?.source, data.profile.site?.reported_at_ms)}
          {canWrite && data.defaults.site && <button type="button" className="link-button" onClick={() => { update(applyDefaults(form, { ...data.defaults, optics: null })); setNotice(''); }}><Download size={14} />Use frame headers</button>}
        </p>
        <div className="rig-profile-grid">
          <Field id={`${slug}-latitude`} label="Latitude" unit="°" value={form.latitude} disabled={disabled} onChange={latitude => update({ latitude, siteSource: { kind: 'manual' } })} />
          <Field id={`${slug}-longitude`} label="Longitude (east +)" unit="°" value={form.longitude} disabled={disabled} onChange={longitude => update({ longitude, siteSource: { kind: 'manual' } })} />
          <Field id={`${slug}-elevation`} label="Elevation" unit="m" value={form.elevation} disabled={disabled} onChange={elevation => update({ elevation, siteSource: { kind: 'manual' } })} />
          <Field id={`${slug}-bortle`} label="Bortle class" step="1" value={form.bortle} disabled={disabled} onChange={bortle => update({ bortle })} />
          <Field id={`${slug}-sqm`} label="Sky brightness" unit="mag/arcsec²" value={form.sqm} disabled={disabled} onChange={sqm => update({ sqm })} />
        </div>
        <HorizonEditor label="Rig horizon" name={data.rig.name} horizon={form.horizon?.value ?? null} disabled={disabled}
          note={form.horizon
            ? (form.horizon.source.kind === data.profile.horizon?.source.kind && form.horizon.value === data.profile.horizon?.value
              ? describeSource(data.profile.horizon.source, data.profile.horizon.reported_at_ms).toLowerCase()
              : 'not saved yet')
            : placed?.horizon_from === 'site' ? `Uses the horizon for ${placed.site?.name ?? 'the site'}` : 'No rig horizon set'}
          onChange={horizon => update({ horizon: horizon ? { value: horizon, source: { kind: 'manual' } } : null })} />
      </fieldset>
      </div>
      <div {...workspacePanel(id, 'limits', tab)}>
      <fieldset disabled={disabled}>
        <legend>Planning limits</legend>
        <div className="rig-profile-grid">
          <Field id={`${slug}-min-alt`} label="Minimum altitude" unit="°" value={form.minAltitude} disabled={disabled} onChange={minAltitude => update({ minAltitude })} />
          <Field id={`${slug}-max-alt`} label="Maximum altitude" unit="°" value={form.maxAltitude} disabled={disabled} onChange={maxAltitude => update({ maxAltitude })} />
          <Field id={`${slug}-meridian-before`} label="Stop before meridian" unit="min" value={form.meridianBefore} disabled={disabled} onChange={meridianBefore => update({ meridianBefore })} />
          <Field id={`${slug}-meridian-after`} label="Resume after meridian" unit="min" value={form.meridianAfter} disabled={disabled} onChange={meridianAfter => update({ meridianAfter })} />
        </div>
      </fieldset>
      <fieldset disabled={disabled} className="scheduling-defaults">
        <legend>Target Scheduler limits</legend>
        <p className="director-muted">A plan's own limits override these.</p>
        <SchedulingFields label="Rig Target Scheduler limits" overrides={scheduling} onChange={next => editSettings({ scheduling: compactOverrides(next) })}
          inherited={parentLimits.values} inheritedFrom={limit => parentLimits.from[limit]} disabled={disabled || !rigSettings.data} />
      </fieldset>
      {rigId && <RigSchedulingApply rigId={rigId} unsaved={schedulingChanged} />}
      <fieldset disabled={disabled}>
        <legend>Remote site</legend>
        <p className="director-muted">Activation updates the local copy, then sends the changes to the chosen server through Sync.</p>
        <label className="rig-profile-field" htmlFor={`${slug}-peer`}><span>Send plans to</span>
          <span className="rig-profile-input">
            <select id={`${slug}-peer`} aria-label="Send plans to" value={form.peerId} onChange={event => update({ peerId: event.target.value })}>
              <option value="">This server only</option>
              {peers.data?.map(peer => <option key={peer.id} value={peer.id}>{peer.name}</option>)}
              {form.peerId && !peers.data?.some(peer => peer.id === form.peerId) && <option value={form.peerId}>{form.peerId} (no longer registered)</option>}
            </select>
          </span>
        </label>
        {peers.isError && <p className="director-muted">Peers could not be loaded. Check Settings, Remote PSF Guard.</p>}
        {peers.data?.length === 0 && <p className="director-muted">No peer registered yet; add one under Settings, Remote PSF Guard.</p>}
      </fieldset>
      <p className="director-muted">Camera modes and filters: {data.profile.configuration ? describeSource(data.profile.configuration.source, data.profile.configuration.reported_at_ms) : 'not reported; only the N.I.N.A. Director plugin reports them.'} <EarlyBeta /></p>
      </div>
      {(problem || save.isError) && !stale && <p className="director-error" role="alert">{problem || message(save.error)}</p>}
      {canWrite && <div className="director-actions"><button type="submit" disabled={disabled}><Check size={16} />{save.isPending ? 'Saving...' : 'Save rig profile'}</button>{unsaved && !save.isPending && <span className="unsaved-note">Unsaved changes</span>}</div>}
      {!canWrite && <p className="director-muted">Read only</p>}
    </form>
    <div {...workspacePanel(id, 'weather', tab)}>{tab === 'weather' && <RigWeatherPanel rig={data.rig.id} />}</div>
    <div {...workspacePanel(id, 'director', tab)}>{tab === 'director' && syncPlugin && <>{syncPlugin}<p className="director-muted">The Director plugin below has its own pairing code. Both plugins can run on this rig.</p></>}{tab === 'director' && <RigPluginPanel slug={slug} mode={data.mode.runtime} onRigChanged={() => { if (!unsaved) void loaded.refetch(); }} onChangeMode={() => setTab('optics')} />}</div>
    <div {...workspacePanel(id, 'collaboration', tab)}>{data && collaborationVisited && <CollaborationConnections rig={data.rig.id} name={data.rig.name} active={tab === 'collaboration'} />}</div>
    </div>
    </div>}
  </section>;
}

/** Where planning takes this rig's location and horizon from. */
function describePlacement(placed: DirectorRigSite | undefined): string {
  if (!placed) return 'Planning reads the location and horizon once the rig list loads.';
  const from = (origin: DirectorRigSite['location_from']) => origin === 'rig' ? 'this rig' : origin === 'site' ? (placed.site?.name ?? 'its site') : null;
  const location = from(placed.location_from);
  const horizon = from(placed.horizon_from);
  return `Planning uses ${location ? `the location from ${location}` : 'no location yet'} and ${horizon ? `the horizon from ${horizon}` : 'a flat horizon at the minimum altitude'}.`;
}
