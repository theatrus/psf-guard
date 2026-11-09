import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, ChevronDown, ChevronRight, CloudDownload, Plus } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorHorizon, DirectorIdentity, DirectorSiteProfileView, ForecastProvider } from '../../api/directorTypes';
import { DEFAULT_CLOUD_THRESHOLD, MAX_DEW_LIMIT, PROVIDER_LABEL, PROVIDER_URL } from './forecastLabels';
import HorizonEditor from './HorizonEditor';
import { identityId } from './identityId';
import { describeSource } from './rigProfileForm';
import { retryWhenBusy } from './retry';
import { useUnsavedChanges } from '../../hooks/useUnsavedChanges';

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Site request failed';

/** Observing sites: where rigs stand and what blocks their sky. A rig that
 *  names a site as its planning site takes both. `focusSite` opens that
 *  site and moves focus to its weather forecast settings. */
export default function DirectorSites({ focusSite = null }: { focusSite?: string | null } = {}) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const sites = useQuery({ queryKey: ['directorSites'], queryFn: () => apiClient.getDirectorIdentities('sites'), retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const [open, setOpen] = useState<string | null>(null);
  // A site once opened stays mounted, hidden while folded, so opening
  // another keeps its unsaved edits (and Settings still asks before closing).
  const [opened, setOpened] = useState<ReadonlySet<string>>(() => new Set());
  const show = (id: string | null) => {
    setOpen(id);
    if (id) setOpened(current => current.has(id) ? current : new Set(current).add(id));
  };
  const focusable = !!focusSite && !!sites.data?.items.some(site => site.id === focusSite);
  useEffect(() => { if (focusable && focusSite) show(focusSite); }, [focusable, focusSite]);
  const [name, setName] = useState('');
  const create = useMutation({
    mutationFn: (siteName: string) => apiClient.createDirectorIdentity('sites', { id: identityId(), name: siteName }),
    onSuccess: site => {
      setName('');
      show(site.id);
      void client.invalidateQueries({ queryKey: ['directorSites'] });
      void client.invalidateQueries({ queryKey: ['observingDefaults'] });
    },
  });
  const add = (event: FormEvent) => {
    event.preventDefault();
    if (name.trim() && !create.isPending) create.mutate(name.trim());
  };
  return <section aria-label="Sites" className="director-records director-sites">
    <div className="director-toolbar"><h2>Sites</h2></div>
    <p className="director-muted">Choose a rig's planning site under Rig setup → Site.</p>
    {sites.isPending && <p role="status">Loading sites...</p>}
    {sites.isError && <div role="alert"><p>{message(sites.error)}</p><button type="button" onClick={() => void sites.refetch()}>Retry</button></div>}
    {sites.data?.items.length === 0 && <p className="director-muted">No sites yet.</p>}
    <ul className="director-list">{sites.data?.items.map(site => <li key={site.id}>
      <div className="director-record director-record-wide">
        <div className="director-record-name"><strong>{site.name}</strong></div>
        <button type="button" aria-expanded={open === site.id} aria-label={`Edit ${site.name}`} title={`Edit ${site.name}`} onClick={() => show(open === site.id ? null : site.id)}>{open === site.id ? <ChevronDown size={16} /> : <ChevronRight size={16} />}Edit</button>
      </div>
      {(open === site.id || opened.has(site.id)) && <div hidden={open !== site.id}><SiteProfileCard key={site.id} site={site} focusForecast={focusable && focusSite === site.id} /></div>}
    </li>)}</ul>
    {canWrite && <form className="director-site-new" onSubmit={add}>
      <input aria-label="New site name" placeholder="Site name" value={name} maxLength={200} onChange={event => setName(event.target.value)} />
      <button type="submit" disabled={!name.trim() || create.isPending}><Plus size={16} />Add site</button>
      {create.isError && <span className="director-error" role="alert">{message(create.error)}</span>}
    </form>}
  </section>;
}

interface SiteForm {
  name: string; latitude: string; longitude: string; elevation: string; horizon: DirectorHorizon | null;
  forecast: ForecastProvider | 'off';
  threshold: string;
  /** °C above the dew point, or empty to leave dew out. */
  dew: string;
  /** An IANA zone, or empty for the browser's. */
  timeZone: string;
}

/** Every zone the browser knows, for the site's clock. */
const ZONES: string[] = typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : [];
const BROWSER_ZONE = Intl.DateTimeFormat().resolvedOptions().timeZone;

/** What each provider offers, for the choice. */
const PROVIDER_FACTS: Record<ForecastProvider, { range: string; use: string }> = {
  open_meteo: { range: 'Hourly, 16 days', use: 'Free for non-commercial use' },
  met_norway: { range: 'Hourly, then 6-hourly; 9 days', use: 'Free for any use' },
};

/** The queries a site's forecast changes. */
const FORECAST_QUERIES = [['nightPlans'], ['nightPlanDetail'], ['nightPlansTonight'], ['directorProjection'], ['directorFeasibility']] as const;

const text = (value: number | undefined) => value === undefined ? '' : String(value);
const num = (value: string) => value.trim() === '' ? null : Number(value);

function formFrom(view: DirectorSiteProfileView): SiteForm {
  const location = view.profile.location?.value;
  const forecast = view.profile.forecast;
  return {
    name: view.site.name, latitude: text(location?.latitude_degrees), longitude: text(location?.longitude_degrees), elevation: text(location?.elevation_meters), horizon: view.profile.horizon?.value ?? null,
    forecast: forecast?.provider ?? 'off',
    threshold: String(forecast?.cloud_threshold_percent ?? DEFAULT_CLOUD_THRESHOLD),
    dew: text(forecast?.dew_limit_celsius),
    timeZone: view.profile.time_zone ?? '',
  };
}

function SiteProfileCard({ site, focusForecast = false }: { site: DirectorIdentity; focusForecast?: boolean }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const queryKey = ['directorSiteProfile', site.id];
  const loaded = useQuery({ queryKey, queryFn: () => apiClient.getDirectorSiteProfile(site.id), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const [form, setForm] = useState<SiteForm | null>(null);
  const [problem, setProblem] = useState('');
  const [notice, setNotice] = useState('');
  useEffect(() => { if (loaded.data) setForm(formFrom(loaded.data)); }, [loaded.data]);
  const formReady = form !== null;
  const forecastFocused = useRef(false);
  useEffect(() => {
    if (!focusForecast || !formReady || forecastFocused.current) return;
    const select = document.getElementById(`${site.id}-forecast`);
    if (!select) return;
    forecastFocused.current = true;
    select.scrollIntoView?.({ block: 'center' });
    select.focus();
  }, [focusForecast, formReady, site.id]);
  // A rename the last Save got through before its profile step failed: the
  // next Save starts from it rather than renaming again and conflicting.
  const renamed = useRef<DirectorIdentity | null>(null);
  const save = useMutation({
    retry: false,
    mutationFn: async ({ view, next }: { view: DirectorSiteProfileView; next: SiteForm }) => {
      const [lat, lon, elev] = [next.latitude, next.longitude, next.elevation].map(num);
      const located = lat !== null && lon !== null;
      const manual = { kind: 'manual' as const };
      const stored = view.profile;
      let identity = renamed.current ?? view.site;
      if (next.name.trim() !== identity.name) {
        identity = await apiClient.renameDirectorIdentity('sites', identity.id, { expected_revision: identity.revision, name: next.name.trim() });
        renamed.current = identity;
        void client.invalidateQueries({ queryKey: ['directorSites'] });
      }
      const saved = await apiClient.saveDirectorSiteProfile(site.id, {
        expected_revision: stored.revision,
        location: located ? { value: { latitude_degrees: lat, longitude_degrees: lon, elevation_meters: elev ?? 0 }, source: manual } : null,
        horizon: next.horizon ? { value: next.horizon, source: stored.horizon && stored.horizon.value === next.horizon ? stored.horizon.source : manual } : null,
        forecast: next.forecast === 'off' ? null : {
          provider: next.forecast,
          cloud_threshold_percent: Number(next.threshold),
          ...(next.dew.trim() === '' ? {} : { dew_limit_celsius: Number(next.dew) }),
        },
        time_zone: next.timeZone || null,
      });
      return { ...saved, site: identity };
    },
    onSuccess: saved => {
      renamed.current = null;
      setNotice(`Saved ${saved.site.name}.`);
      client.setQueryData(queryKey, saved);
      void client.invalidateQueries({ queryKey: ['directorSites'] });
      void client.invalidateQueries({ queryKey: ['observingDefaults'] });
      void client.invalidateQueries({ queryKey: ['directorRigProfiles'] });
      void client.invalidateQueries({ queryKey: ['siteForecast', site.id] });
      for (const key of FORECAST_QUERIES) void client.invalidateQueries({ queryKey: [...key] });
    },
  });
  // The client wraps a reply that carries an error message; the status is on its cause.
  const httpError = isAxiosError(save.error) ? save.error : save.error instanceof Error && isAxiosError(save.error.cause) ? save.error.cause : null;
  const stale = httpError?.response?.status === 409;
  const reload = () => { renamed.current = null; setNotice(''); setProblem(''); save.reset(); void loaded.refetch(); };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!loaded.data || !form || !canWrite || save.isPending) return;
    setNotice(''); setProblem('');
    const [lat, lon, elev] = [form.latitude, form.longitude, form.elevation].map(num);
    if (!form.name.trim()) { setProblem('A site needs a name.'); return; }
    if ((lat === null) !== (lon === null)) { setProblem('Enter both latitude and longitude, or leave both empty.'); return; }
    if ([lat, lon, elev].some(value => value !== null && !Number.isFinite(value))) { setProblem('Latitude, longitude and elevation are numbers.'); return; }
    if (lat !== null && Math.abs(lat) > 90) { setProblem('Latitude runs from -90 to 90.'); return; }
    if (lon !== null && Math.abs(lon) > 180) { setProblem('Longitude runs from -180 to 180, east positive.'); return; }
    if (form.forecast !== 'off') {
      const threshold = Number(form.threshold);
      if (lat === null) { setProblem("A forecast needs the site's location. Enter its latitude and longitude."); return; }
      if (form.threshold.trim() === '' || !Number.isInteger(threshold) || threshold < 0 || threshold > 100) { setProblem('Enter the clear-sky threshold as a whole percent from 0 to 100.'); return; }
      const dew = Number(form.dew);
      if (form.dew.trim() !== '' && (!Number.isInteger(dew) || dew < 0 || dew > MAX_DEW_LIMIT)) { setProblem(`Enter the dew limit as whole degrees from 0 to ${MAX_DEW_LIMIT}, or leave it empty.`); return; }
    }
    save.mutate({ view: loaded.data, next: form });
  };
  const update = (patch: Partial<SiteForm>) => setForm(current => current ? { ...current, ...patch } : current);
  const data = loaded.data;
  // Edits Save has not sent; Settings asks before discarding them. With
  // forecasts off, the thresholds are not saved, so they are not edits.
  const comparable = (value: SiteForm) => value.forecast === 'off' ? { ...value, threshold: '', dew: '' } : value;
  const unsaved = !!data && !!form && canWrite && JSON.stringify(comparable(form)) !== JSON.stringify(comparable(formFrom(data)));
  useUnsavedChanges(`site ${data?.site.name ?? site.name}`, unsaved);
  const disabled = !canWrite || save.isPending || stale;
  const field = (key: 'latitude' | 'longitude' | 'elevation', label: string, unit: string) => <label className="rig-profile-field" htmlFor={`${site.id}-${key}`}><span>{label}</span>
    <span className="rig-profile-input"><input id={`${site.id}-${key}`} aria-label={label} type="number" inputMode="decimal" step="any" value={form?.[key] ?? ''} onChange={event => update({ [key]: event.target.value })} /><small>{unit}</small></span>
  </label>;
  return <section className="rig-profile" aria-label={`Site ${site.name}`}>
    {loaded.isPending && <p role="status">Loading site...</p>}
    {loaded.isError && <p className="director-error" role="alert">{message(loaded.error)}</p>}
    {notice && <p role="status">{notice}</p>}
    {stale && <p className="director-error" role="alert">This site changed since you loaded it. <button type="button" onClick={reload}>Reload</button></p>}
    {data && form && <form onSubmit={submit} className="rig-profile-form">
      <fieldset disabled={disabled}>
        <legend>Location</legend>
        <div className="rig-profile-grid">
          <label className="rig-profile-field" htmlFor={`${site.id}-name`}><span>Name</span>
            <span className="rig-profile-input"><input id={`${site.id}-name`} aria-label="Site name" value={form.name} maxLength={200} onChange={event => update({ name: event.target.value })} /></span>
          </label>
          {field('latitude', 'Latitude', '°')}
          {field('longitude', 'Longitude (east +)', '°')}
          {field('elevation', 'Elevation', 'm')}
          <label className="rig-profile-field" htmlFor={`${site.id}-zone`}><span>Time zone</span>
            <span className="rig-profile-input"><select id={`${site.id}-zone`} aria-label="Time zone" aria-describedby={`${site.id}-zone-help`} value={form.timeZone} onChange={event => update({ timeZone: event.target.value })}>
              <option value="">This browser's ({BROWSER_ZONE})</option>
              {form.timeZone && !ZONES.includes(form.timeZone) && <option value={form.timeZone}>{form.timeZone}</option>}
              {ZONES.map(zone => <option key={zone} value={zone}>{zone}</option>)}
            </select></span>
          </label>
        </div>
        <p id={`${site.id}-zone-help`} className="director-muted">Night marks and overrides for rigs at this site use this time zone.</p>
        {data.profile.location && <p className="director-muted">{describeSource(data.profile.location.source, data.profile.location.reported_at_ms)}</p>}
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Horizon</legend>
        <HorizonEditor label="Site horizon" name={form.name} horizon={form.horizon} disabled={disabled}
          note={form.horizon && form.horizon === data.profile.horizon?.value ? describeSource(data.profile.horizon.source, data.profile.horizon.reported_at_ms).toLowerCase() : form.horizon ? 'not saved yet' : undefined}
          onChange={horizon => update({ horizon })} />
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Weather forecast</legend>
        <div className="rig-profile-grid">
          <label className="rig-profile-field" htmlFor={`${site.id}-forecast`}><span>Forecast</span>
            <span className="rig-profile-input"><select id={`${site.id}-forecast`} aria-label="Forecast" value={form.forecast} onChange={event => update({ forecast: event.target.value as SiteForm['forecast'] })}>
              <option value="off">Off</option>
              {(Object.keys(PROVIDER_LABEL) as ForecastProvider[]).map(provider => <option key={provider} value={provider}>{PROVIDER_LABEL[provider]}</option>)}
            </select></span>
          </label>
          {form.forecast !== 'off' && <label className="rig-profile-field" htmlFor={`${site.id}-threshold`}><span>Clear at or under</span>
            <span className="rig-profile-input"><input id={`${site.id}-threshold`} aria-label="Clear at or under" type="number" inputMode="numeric" min={0} max={100} step={1} value={form.threshold} onChange={event => update({ threshold: event.target.value })} /><small>% cloud</small></span>
          </label>}
          {form.forecast !== 'off' && <label className="rig-profile-field" htmlFor={`${site.id}-dew`}><span>No imaging within</span>
            <span className="rig-profile-input"><input id={`${site.id}-dew`} aria-label="No imaging within" type="number" inputMode="numeric" min={0} max={MAX_DEW_LIMIT} step={1} placeholder="Off" value={form.dew} onChange={event => update({ dew: event.target.value })} /><small>°C of dew point</small></span>
          </label>}
        </div>
        <dl className="site-forecast-facts">{form.forecast === 'off'
          ? <div><dt>Sends</dt><dd>Nothing</dd></div>
          : <>
            <div><dt>Range</dt><dd>{PROVIDER_FACTS[form.forecast].range}</dd></div>
            <div><dt>Use</dt><dd>{PROVIDER_FACTS[form.forecast].use}</dd></div>
            <div><dt>Sends</dt><dd>Latitude and longitude, to about 1 km</dd></div>
            <div><dt>When</dt><dd>On save, Fetch now, every 3 hours, after a failure</dd></div>
          </>}</dl>
        {data.profile.forecast && <SiteForecastStatus site={site} />}
      </fieldset>
      {(problem || (save.isError && !stale)) && <p className="director-error" role="alert">{problem || message(save.error)}</p>}
      {canWrite && <div className="director-actions"><button type="submit" disabled={disabled}><Check size={16} />{save.isPending ? 'Saving...' : 'Save site'}</button>{unsaved && !save.isPending && <span className="unsaved-note">Unsaved changes</span>}</div>}
      {!canWrite && <p className="director-muted">Read only</p>}
    </form>}
  </section>;
}

const when = (ms: number) => new Date(ms).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });

/** The site's forecast: when it arrived, until when night plans use it,
 *  the last failed request, and Fetch now. */
function SiteForecastStatus({ site }: { site: DirectorIdentity }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const queryKey = ['siteForecast', site.id];
  const view = useQuery({
    queryKey,
    queryFn: () => apiClient.getSiteForecast(site.id),
    retry: retryWhenBusy,
    retryDelay: 1200,
    refetchOnWindowFocus: false,
    // The server requests the first forecast after Save: poll until it
    // arrives, for a minute at most.
    refetchInterval: query => {
      const data = query.state.data;
      return data?.settings && !data.forecast && !data.status && query.state.dataUpdateCount < 30 ? 2000 : 60_000;
    },
  });
  // A forecast that arrives or changes changes the night plans too.
  const arrived = view.data?.forecast ? `${view.data.forecast.fetched_at_ms}:${view.data.forecast.checked_at_ms ?? ''}` : '';
  const seen = useRef(arrived);
  useEffect(() => {
    if (arrived === seen.current) return;
    seen.current = arrived;
    for (const key of FORECAST_QUERIES) void client.invalidateQueries({ queryKey: [...key] });
  }, [arrived, client]);
  const fetchNow = useMutation({
    mutationFn: () => apiClient.refreshSiteForecast(site.id),
    onSuccess: data => {
      client.setQueryData(queryKey, data);
      for (const key of FORECAST_QUERIES) void client.invalidateQueries({ queryKey: [...key] });
    },
  });
  const data = view.data;
  if (view.isPending) return <p role="status">Loading the forecast...</p>;
  if (view.isError || !data) return <p className="director-error" role="alert">{message(view.error)}</p>;
  const { forecast, status } = data;
  const last = forecast?.hours.at(-1);
  return <div className="site-forecast-status" role="group" aria-label="Forecast status">
    {forecast
      ? <dl className="site-forecast-facts">
        <div><dt>Fetched</dt><dd>{when(forecast.fetched_at_ms)}</dd></div>
        {forecast.checked_at_ms !== null && <div><dt>Unchanged at</dt><dd>{when(forecast.checked_at_ms)}</dd></div>}
        {last && <div><dt>Covers until</dt><dd>{when(last.end_ms)}</dd></div>}
        <div><dt>Plans use it</dt><dd>{data.fresh ? `Until ${when(forecast.expires_at_ms)}` : 'Expired: nights count as clear'}</dd></div>
        {data.next_refresh_ms !== null && <div><dt>Next refresh</dt><dd>{when(data.next_refresh_ms)}</dd></div>}
      </dl>
      : <p>{status ? 'No forecast yet' : 'Requesting the first forecast...'}</p>}
    {status?.last_error && <p className="director-error" role="alert">Request at {when(status.last_attempt_ms)} failed: {status.last_error}{forecast && data.fresh ? ' Plans still use the stored forecast.' : ''}</p>}
    {!forecast && data.next_refresh_ms !== null && <p className="director-muted">Next refresh: {when(data.next_refresh_ms)}</p>}
    {canWrite && <div className="director-actions">
      <button type="button" disabled={fetchNow.isPending} onClick={() => fetchNow.mutate()}><CloudDownload size={16} />{fetchNow.isPending ? 'Fetching...' : 'Fetch now'}</button>
      {fetchNow.isError && <span className="director-error" role="alert">{message(fetchNow.error)}</span>}
    </div>}
    {data.attribution && data.settings && <p className="director-muted"><a href={PROVIDER_URL[data.settings.provider]} target="_blank" rel="noreferrer">{data.attribution}</a></p>}
  </div>;
}
