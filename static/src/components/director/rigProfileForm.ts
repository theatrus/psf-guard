import type { DirectorEdited, DirectorRigFilters, DirectorFieldOfView, DirectorHorizon, DirectorOptics, DirectorReported, DirectorRigMode, DirectorRigProfile, DirectorRigProfileEdit, DirectorRigProfileView, DirectorSource } from '../../api/directorTypes';

/** Text fields for every number the form edits, so a half-typed value never throws. */
export interface RigProfileForm {
  sensorWidth: string; sensorHeight: string; pixelSize: string; focalLength: string; aperture: string;
  rotationMode: 'fixed' | 'manual' | 'rotator'; rotationAngle: string;
  opticsSource: DirectorSource | null;
  latitude: string; longitude: string; elevation: string; siteSource: DirectorSource | null;
  bortle: string; sqm: string;
  minAltitude: string; maxAltitude: string; meridianBefore: string; meridianAfter: string;
  /** Registered peer id, or '' when the rig executes from this server. */
  peerId: string;
  /** The rig's own horizon, which wins over its site's; null inherits. */
  horizon: DirectorEdited<DirectorHorizon> | null;
  /** Who picks the rig's work; '' leaves it to PSF Guard's guess. */
  mode: DirectorRigMode | '';
  /** Typed filter names, one per line; '' has no list. */
  filters: string;
}

const text = (value: number | null | undefined) => value === null || value === undefined ? '' : String(value);

export function formFromProfile(profile: DirectorRigProfile): RigProfileForm {
  const optics = profile.optics?.value;
  const rotation = optics?.rotation;
  const site = profile.site?.value;
  const limits = profile.limits.value;
  return {
    sensorWidth: text(optics?.sensor_width_px), sensorHeight: text(optics?.sensor_height_px),
    pixelSize: text(optics?.pixel_size_um), focalLength: text(optics?.focal_length_mm), aperture: text(optics?.aperture_mm),
    rotationMode: rotation?.mode ?? 'manual', rotationAngle: rotation && rotation.mode !== 'rotator' ? text(rotation.angle_degrees) : '0',
    opticsSource: profile.optics?.source ?? null,
    latitude: text(site?.latitude_degrees), longitude: text(site?.longitude_degrees), elevation: text(site?.elevation_meters),
    siteSource: profile.site?.source ?? null,
    bortle: text(profile.sky_quality?.value.bortle_class), sqm: text(profile.sky_quality?.value.sqm_mag_per_arcsec2),
    minAltitude: text(limits.minimum_altitude_degrees), maxAltitude: text(limits.maximum_altitude_degrees),
    meridianBefore: text(limits.meridian_exclusion.before_ms / 60000), meridianAfter: text(limits.meridian_exclusion.after_ms / 60000),
    peerId: profile.peer_id ?? '',
    horizon: profile.horizon ? { value: profile.horizon.value, source: profile.horizon.source } : null,
    mode: profile.mode ?? '',
    filters: profile.filters?.value.join('\n') ?? '',
  };
}

/** The typed names, one per line, as the server stores them: trimmed, no blanks, each once regardless of case. */
export function filterNames(text: string): string[] {
  const seen = new Set<string>();
  return text.split(/\r?\n/).map(name => name.trim()).filter(name => {
    const key = name.toLowerCase();
    if (!name || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** Whether the rig can shoot a template's filter, by the server's rule: Target
 *  Scheduler needs the name itself; a Director plugin's wheel also serves the
 *  one filter of the template's bandpass. True when the rig has no list. */
export function hasFilter(list: DirectorRigFilters | null | undefined, filter: string, bandpass: string): boolean {
  if (!list) return true;
  const same = (name: string) => name.trim().toLowerCase() === filter.trim().toLowerCase();
  if (!list.reported) return list.filters.some(entry => same(entry.name));
  const exact = list.filters.filter(entry => same(entry.name));
  return (exact.length > 0 ? exact : list.filters.filter(entry => entry.bandpass === bandpass)).length === 1;
}

/** Bytes a name takes in UTF-8, as the server measures it. */
const utf8Length = (name: string) => new TextEncoder().encode(name).length;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

/** Copy the header-derived optics and site into the form, keeping their source. */
export function applyDefaults(form: RigProfileForm, defaults: DirectorRigProfileView['defaults']): RigProfileForm {
  const next = { ...form };
  if (defaults.optics) {
    const optics = defaults.optics.value;
    next.sensorWidth = text(optics.sensor_width_px); next.sensorHeight = text(optics.sensor_height_px);
    next.pixelSize = text(optics.pixel_size_um); next.focalLength = text(optics.focal_length_mm); next.aperture = text(optics.aperture_mm);
    next.opticsSource = defaults.optics.source;
  }
  if (defaults.site) {
    const site = defaults.site.value;
    next.latitude = text(site.latitude_degrees); next.longitude = text(site.longitude_degrees); next.elevation = text(site.elevation_meters);
    next.siteSource = defaults.site.source;
  }
  return next;
}

const num = (value: string) => value.trim() === '' ? null : Number(value);
const finite = (value: number | null): value is number => value !== null && Number.isFinite(value);

export function opticsFromForm(form: RigProfileForm): DirectorOptics | null {
  const [w, h, p, f, a, angle] = [form.sensorWidth, form.sensorHeight, form.pixelSize, form.focalLength, form.aperture, form.rotationAngle].map(num);
  if (!finite(w) || !finite(h) || !finite(p) || !finite(f)) return null;
  const rotation = form.rotationMode === 'rotator' ? { mode: 'rotator' as const } : { mode: form.rotationMode, angle_degrees: finite(angle) ? angle : 0 };
  return { sensor_width_px: Math.round(w), sensor_height_px: Math.round(h), pixel_size_um: p, focal_length_mm: f, aperture_mm: finite(a) ? a : null, rotation };
}

/** The same small-angle field the server computes, for a live preview while typing. */
export function fieldOfView(optics: DirectorOptics): DirectorFieldOfView | null {
  if (optics.pixel_size_um <= 0 || optics.focal_length_mm <= 0) return null;
  const pixel_scale_arcsec = 206264.80624709636 * optics.pixel_size_um / 1000 / optics.focal_length_mm;
  return {
    width_degrees: optics.sensor_width_px * pixel_scale_arcsec / 3600,
    height_degrees: optics.sensor_height_px * pixel_scale_arcsec / 3600,
    pixel_scale_arcsec,
    focal_ratio: optics.aperture_mm ? optics.focal_length_mm / optics.aperture_mm : null,
  };
}

export function formatFieldOfView(fov: DirectorFieldOfView): string {
  const ratio = fov.focal_ratio ? `, f/${fov.focal_ratio.toFixed(1)}` : '';
  return `${fov.width_degrees.toFixed(2)}° × ${fov.height_degrees.toFixed(2)}°, ${fov.pixel_scale_arcsec.toFixed(2)}″ per pixel${ratio}`;
}

/** Build the request, or name the first field that cannot be sent. */
export function editFromForm(form: RigProfileForm, profile: DirectorRigProfile): DirectorRigProfileEdit | string {
  const manual: DirectorSource = { kind: 'manual' };
  const anyOptics = [form.sensorWidth, form.sensorHeight, form.pixelSize, form.focalLength].some(v => v.trim() !== '');
  const optics = opticsFromForm(form);
  if (anyOptics && !optics) return 'Enter the sensor size, pixel size and focal length together, or leave all of them empty.';
  const [lat, lon, elev] = [form.latitude, form.longitude, form.elevation].map(num);
  const anySite = [form.latitude, form.longitude].some(v => v.trim() !== '');
  if (anySite && (!finite(lat) || !finite(lon))) return 'Enter both latitude and longitude, or leave both empty.';
  const bortle = num(form.bortle);
  const sqm = num(form.sqm);
  if (bortle !== null && (!Number.isInteger(bortle) || bortle < 1 || bortle > 9)) return 'Bortle class is a whole number from 1 to 9.';
  const [minAlt, maxAlt, before, after] = [form.minAltitude, form.maxAltitude, form.meridianBefore, form.meridianAfter].map(num);
  if (!finite(minAlt) || !finite(maxAlt) || maxAlt <= minAlt) return 'The maximum altitude must be above the minimum.';
  if (!finite(before) || !finite(after) || before < 0 || after < 0) return 'Meridian minutes cannot be negative.';
  const filters = filterNames(form.filters);
  const long = filters.find(name => utf8Length(name) > 64);
  if (long) return `The filter name "${long}" is too long. Use at most 64 bytes.`;
  if (filters.some(name => CONTROL.test(name))) return 'Filter names cannot contain tabs or other control characters.';
  if (filters.length > 64) return 'Enter at most 64 filters.';
  const stored = (part: DirectorReported<unknown> | null, source: DirectorSource | null) => source ?? part?.source ?? manual;
  return {
    expected_revision: profile.revision,
    optics: optics ? { value: optics, source: stored(profile.optics, form.opticsSource) } : null,
    site: finite(lat) && finite(lon) ? { value: { latitude_degrees: lat, longitude_degrees: lon, elevation_meters: finite(elev) ? elev : 0 }, source: stored(profile.site, form.siteSource) } : null,
    horizon: form.horizon,
    sky_quality: bortle !== null ? { value: { bortle_class: bortle, sqm_mag_per_arcsec2: finite(sqm) ? sqm : null }, source: manual } : null,
    limits: { value: { minimum_altitude_degrees: minAlt, maximum_altitude_degrees: maxAlt, meridian_exclusion: { before_ms: Math.round(before * 60000), after_ms: Math.round(after * 60000) } }, source: manual },
    peer_id: form.peerId || null,
    mode: form.mode || null,
    filters: filters.length > 0 ? { value: filters, source: manual } : null,
  };
}

export function describeSource(source: DirectorSource | null | undefined, at?: number): string {
  if (!source) return 'Not set';
  const when = at ? ` on ${new Date(at).toLocaleDateString()}` : '';
  switch (source.kind) {
    case 'frame_headers': return `From frame headers of ${source.file_name}${when}`;
    case 'plugin': return `Reported by the N.I.N.A. Director plugin${when}`;
    default: return `Set by hand${when}`;
  }
}
