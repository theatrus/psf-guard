import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, ChevronDown, ChevronRight, Plus } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorHorizon, DirectorIdentity, DirectorSiteProfileView } from '../../api/directorTypes';
import HorizonEditor from './HorizonEditor';
import { identityId } from './identityId';
import { describeSource } from './rigProfileForm';
import { retryWhenBusy } from './retry';

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : 'Site request failed';

/** Observing sites: where rigs stand and what blocks their sky. A rig that
 *  names a site as its planning site takes both. */
export default function DirectorSites() {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const sites = useQuery({ queryKey: ['directorSites'], queryFn: () => apiClient.getDirectorIdentities('sites'), retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false });
  const [open, setOpen] = useState<string | null>(null);
  const [name, setName] = useState('');
  const create = useMutation({
    mutationFn: (siteName: string) => apiClient.createDirectorIdentity('sites', { id: identityId(), name: siteName }),
    onSuccess: site => {
      setName('');
      setOpen(site.id);
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
    <p className="director-muted">A site holds a location and a horizon. Each rig picks its planning site under Setup and takes both, unless it has its own.</p>
    {sites.isPending && <p role="status">Loading sites...</p>}
    {sites.isError && <div role="alert"><p>{message(sites.error)}</p><button type="button" onClick={() => void sites.refetch()}>Retry</button></div>}
    {sites.data?.items.length === 0 && <p className="director-muted">No sites yet.</p>}
    <ul className="director-list">{sites.data?.items.map(site => <li key={site.id}>
      <div className="director-record director-record-wide">
        <div className="director-record-name"><strong>{site.name}</strong></div>
        <button type="button" aria-expanded={open === site.id} aria-label={`Edit ${site.name}`} title={`Edit ${site.name}`} onClick={() => setOpen(open === site.id ? null : site.id)}>{open === site.id ? <ChevronDown size={16} /> : <ChevronRight size={16} />}Edit</button>
      </div>
      {open === site.id && <SiteProfileCard key={site.id} site={site} />}
    </li>)}</ul>
    {canWrite && <form className="director-site-new" onSubmit={add}>
      <input aria-label="New site name" placeholder="Site name" value={name} maxLength={200} onChange={event => setName(event.target.value)} />
      <button type="submit" disabled={!name.trim() || create.isPending}><Plus size={16} />Add site</button>
      {create.isError && <span className="director-error" role="alert">{message(create.error)}</span>}
    </form>}
  </section>;
}

interface SiteForm { name: string; latitude: string; longitude: string; elevation: string; horizon: DirectorHorizon | null }

const text = (value: number | undefined) => value === undefined ? '' : String(value);
const num = (value: string) => value.trim() === '' ? null : Number(value);

function formFrom(view: DirectorSiteProfileView): SiteForm {
  const location = view.profile.location?.value;
  return { name: view.site.name, latitude: text(location?.latitude_degrees), longitude: text(location?.longitude_degrees), elevation: text(location?.elevation_meters), horizon: view.profile.horizon?.value ?? null };
}

function SiteProfileCard({ site }: { site: DirectorIdentity }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const queryKey = ['directorSiteProfile', site.id];
  const loaded = useQuery({ queryKey, queryFn: () => apiClient.getDirectorSiteProfile(site.id), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const [form, setForm] = useState<SiteForm | null>(null);
  const [problem, setProblem] = useState('');
  const [notice, setNotice] = useState('');
  useEffect(() => { if (loaded.data) setForm(formFrom(loaded.data)); }, [loaded.data]);
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
    save.mutate({ view: loaded.data, next: form });
  };
  const update = (patch: Partial<SiteForm>) => setForm(current => current ? { ...current, ...patch } : current);
  const data = loaded.data;
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
        </div>
        {data.profile.location && <p className="director-muted">{describeSource(data.profile.location.source, data.profile.location.reported_at_ms)}</p>}
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Horizon</legend>
        <HorizonEditor label="Site horizon" name={form.name} horizon={form.horizon} disabled={disabled}
          note={form.horizon && form.horizon === data.profile.horizon?.value ? describeSource(data.profile.horizon.source, data.profile.horizon.reported_at_ms).toLowerCase() : form.horizon ? 'not saved yet' : undefined}
          onChange={horizon => update({ horizon })} />
      </fieldset>
      {(problem || (save.isError && !stale)) && <p className="director-error" role="alert">{problem || message(save.error)}</p>}
      {canWrite && <div className="director-actions"><button type="submit" disabled={disabled}><Check size={16} />{save.isPending ? 'Saving...' : 'Save site'}</button></div>}
      {!canWrite && <p className="director-muted">Read only</p>}
    </form>}
  </section>;
}
