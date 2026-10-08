import { useEffect, useId, useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Camera, Check, Download, Link2, MapPin, RefreshCw, SlidersHorizontal } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorRigProfileView, DirectorRigSite } from '../../api/directorTypes';
import HorizonEditor from './HorizonEditor';
import CollaborationConnections from './CollaborationConnections';
import WorkspaceTabs from './WorkspaceTabs';
import { workspacePanel } from './workspacePanel';
import { applyDefaults, describeSource, editFromForm, fieldOfView, formFromProfile, formatFieldOfView, opticsFromForm, type RigProfileForm } from './rigProfileForm';

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Rig profile request failed';

const TABS = [{ id: 'optics', label: 'Optics', icon: <Camera size={16} /> }, { id: 'site', label: 'Site', icon: <MapPin size={16} /> }, { id: 'limits', label: 'Limits and delivery', icon: <SlidersHorizontal size={16} /> }, { id: 'collaboration', label: 'Collaboration', icon: <Link2 size={16} /> }] as const;
type Tab = (typeof TABS)[number]['id'];

function Field({ id, label, value, onChange, disabled, unit, step = 'any' }: {
  id: string; label: string; value: string; onChange: (value: string) => void; disabled: boolean; unit?: string; step?: string;
}) {
  return <label className="rig-profile-field" htmlFor={id}>
    <span>{label}</span>
    <span className="rig-profile-input"><input id={id} aria-label={label} type="number" inputMode="decimal" step={step} value={value} disabled={disabled} onChange={event => onChange(event.target.value)} />{unit && <small>{unit}</small>}</span>
  </label>;
}

/** Optics, site and limits for the rig behind one database. */
export default function RigProfileCard({ slug }: { slug: string }) {
  const id = useId();
  const [tab, setTab] = useState<Tab>('optics');
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
  const loaded = useQuery({ queryKey, queryFn: () => apiClient.getDirectorRigProfile(slug), refetchOnWindowFocus: false,
    // Director admits one metadata request at a time; a 503 means wait, not fail.
    retry: (count, error) => count < 5 && (isAxiosError(error) ? error.response?.status : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined) === 503, retryDelay: 700 });
  const peers = useQuery({ queryKey: ['peers'], queryFn: apiClient.getPeers, retry: false, refetchOnWindowFocus: false });
  // Where the rig stands for planning: the site it names, and what that
  // gives it. The rig's own location and horizon still win.
  const rigId = loaded.data?.rig.id;
  const sites = useQuery({ queryKey: ['observingDefaults'], queryFn: apiClient.getObservingDefaults, refetchOnWindowFocus: false });
  const rigSettings = useQuery({ queryKey: ['observingSettings', 'rig', rigId], queryFn: () => apiClient.getObservingSettings('rig', rigId!), enabled: !!rigId, refetchOnWindowFocus: false });
  const summaries = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, refetchOnWindowFocus: false });
  const placed = summaries.data?.find(entry => entry.catalog_slug === slug)?.site;
  const [form, setForm] = useState<RigProfileForm | null>(null);
  const [siteId, setSiteId] = useState<string | null>(null);
  const [problem, setProblem] = useState('');
  const [notice, setNotice] = useState('');
  useEffect(() => { if (loaded.data) setForm(formFromProfile(loaded.data.profile)); }, [loaded.data]);
  useEffect(() => { if (rigSettings.data) setSiteId(rigSettings.data.site_id); }, [rigSettings.data]);
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
      // The planning site lives with the rig's observing settings.
      if (rigSettings.data && siteId !== rigSettings.data.site_id) {
        try {
          const settings = await apiClient.saveObservingSettings({ ...rigSettings.data, site_id: siteId });
          client.setQueryData(['observingSettings', 'rig', settings.scope_id], settings);
          void client.invalidateQueries({ queryKey: ['observingEffective'] });
        } catch (error) {
          throw new Error(`Profile saved, planning site not: ${message(error)}`, { cause: error });
        }
      }
      return saved;
    },
    onSuccess: saved => setNotice(`Saved rig profile revision ${saved.profile.revision}.`),
    // Either step may have changed what planning reads for the rig.
    onSettled: () => void client.invalidateQueries({ queryKey: ['directorRigProfiles'] }),
  });
  // Derived from the error itself, so the conflict notice and the generic one
  // can never both render for the same failed save.
  const httpError = isAxiosError(save.error) ? save.error
    : save.error instanceof Error && isAxiosError(save.error.cause) ? save.error.cause : null;
  const stale = httpError?.response?.status === 409;
  const reload = () => { setNotice(''); setProblem(''); save.reset(); void loaded.refetch(); if (rigId) void rigSettings.refetch(); };
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
  const disabled = !canWrite || save.isPending || stale;
  const optics = form ? opticsFromForm(form) : null;
  const preview = optics ? fieldOfView(optics) : null;
  return <section className="rig-profile" aria-label="Rig profile">
    <div className="director-toolbar rig-profile-heading"><h4>Setup</h4>
      <button type="button" aria-label="Reload rig profile" title="Reload rig profile" disabled={loaded.isFetching || save.isPending} onClick={reload}><RefreshCw size={16} /></button>
    </div>
    {loaded.isPending && <p role="status">Loading rig profile...</p>}
    {loaded.isError && <p className="director-error" role="alert">{message(loaded.error)}</p>}
    {notice && <p role="status">{notice}</p>}
    {stale && <p className="director-error" role="alert">This rig changed since you loaded it. <button type="button" onClick={reload}>Reload</button></p>}
    {data && form && <div className="rig-setup-layout">
    <WorkspaceTabs id={id} className="rig-setup-nav" orientation={vertical ? 'vertical' : 'horizontal'} label="Rig setup sections" tabs={TABS} value={tab} onChange={next => { setTab(next); if (next === 'collaboration') setCollaborationVisited(true); }} />
    <div className="rig-setup-content">
    <form onSubmit={submit} className="rig-profile-form" hidden={tab === 'collaboration'}>
      <div {...workspacePanel(id, 'optics', tab)}>
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
        <p className="director-muted" data-testid="rig-profile-fov">{preview ? `Field ${formatFieldOfView(preview)}` : 'Field of view appears once the sensor, pixel size and focal length are set.'}</p>
      </fieldset>
      </div>
      <div {...workspacePanel(id, 'site', tab)}>
      <fieldset disabled={disabled}>
        <legend>Site</legend>
        <label className="rig-profile-field" htmlFor={`${slug}-planning-site`}><span>Planning site</span>
          <span className="rig-profile-input">
            <select id={`${slug}-planning-site`} aria-label="Planning site" value={siteId ?? ''} disabled={!rigSettings.data} onChange={event => setSiteId(event.target.value || null)}>
              <option value="">None</option>
              {sites.data?.sites.map(site => <option key={site.id} value={site.id}>{site.name}</option>)}
            </select>
          </span>
        </label>
        <p className="director-muted" data-testid="rig-site-origin">{describePlacement(placed)}</p>
        <p className="director-muted">The rig takes its site's location and horizon. Type a location or give a horizon here only when this rig differs; leave them empty to use the site's.</p>
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
            : placed?.horizon_from === 'site' ? `none of its own, so ${placed.site?.name ?? 'the site'}'s applies` : 'none of its own'}
          onChange={horizon => update({ horizon: horizon ? { value: horizon, source: { kind: 'manual' } } : null })} />
      </fieldset>
      </div>
      <div {...workspacePanel(id, 'limits', tab)}>
      <fieldset disabled={disabled}>
        <legend>Limits</legend>
        <div className="rig-profile-grid">
          <Field id={`${slug}-min-alt`} label="Minimum altitude" unit="°" value={form.minAltitude} disabled={disabled} onChange={minAltitude => update({ minAltitude })} />
          <Field id={`${slug}-max-alt`} label="Maximum altitude" unit="°" value={form.maxAltitude} disabled={disabled} onChange={maxAltitude => update({ maxAltitude })} />
          <Field id={`${slug}-meridian-before`} label="Stop before meridian" unit="min" value={form.meridianBefore} disabled={disabled} onChange={meridianBefore => update({ meridianBefore })} />
          <Field id={`${slug}-meridian-after`} label="Resume after meridian" unit="min" value={form.meridianAfter} disabled={disabled} onChange={meridianAfter => update({ meridianAfter })} />
        </div>
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Remote site</legend>
        <p className="director-muted">When this rig's real database lives on another PSF Guard, name that peer. Activation writes the plan into the copy here, then pushes the same rows there through Sync.</p>
        <label className="rig-profile-field" htmlFor={`${slug}-peer`}><span>Plans push to</span>
          <span className="rig-profile-input">
            <select id={`${slug}-peer`} aria-label="Plans push to" value={form.peerId} onChange={event => update({ peerId: event.target.value })}>
              <option value="">This server only</option>
              {peers.data?.map(peer => <option key={peer.id} value={peer.id}>{peer.name}</option>)}
              {form.peerId && !peers.data?.some(peer => peer.id === form.peerId) && <option value={form.peerId}>{form.peerId} (no longer registered)</option>}
            </select>
          </span>
        </label>
        {peers.isError && <p className="director-muted">Peers could not be loaded; they are kept under Settings, Remote PSF Guard.</p>}
        {peers.data?.length === 0 && <p className="director-muted">No peer registered yet; add one under Settings, Remote PSF Guard.</p>}
      </fieldset>
      <p className="director-muted">Camera modes and filters: {data.profile.configuration ? describeSource(data.profile.configuration.source, data.profile.configuration.reported_at_ms) : 'not reported yet; the N.I.N.A. plugin supplies them.'}</p>
      </div>
      {(problem || save.isError) && !stale && <p className="director-error" role="alert">{problem || message(save.error)}</p>}
      {canWrite && <div className="director-actions"><button type="submit" disabled={disabled}><Check size={16} />{save.isPending ? 'Saving...' : 'Save rig profile'}</button></div>}
      {!canWrite && <p className="director-muted">Read only</p>}
    </form>
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
