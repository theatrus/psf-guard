import type { NightPlans, RigNight, RigNights } from '../../../api/nightPlanTypes';

const HOUR = 3_600_000;
/** Local noon on 3 November 2026 in the test's clock. */
export const NOON = new Date(2026, 10, 3, 12, 0).getTime();

export function night(index: number, overrides: Partial<RigNight> = {}): RigNight {
  const noon = NOON + index * 24 * HOUR;
  const date = new Date(noon);
  const pad = (value: number) => String(value).padStart(2, '0');
  return {
    date: `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
    noon_ms: noon,
    dusk_ms: noon + 7 * HOUR,
    dawn_ms: noon + 17 * HOUR,
    dark_hours: 10,
    available_dark_hours: 10,
    availability: { state: 'available', windows: [], marks: [] },
    moon_illumination: 0.42,
    moon_hours_up_in_dark: 3,
    targets: [{ project_id: 7, target_id: 70, hours_up: 6, hours_up_available: 6, hours_moon_blocked: 1, max_altitude_degrees: 71.2, transit_ms: noon + 12 * HOUR }],
    load: { demand_hours: 5, available_hours: 10, idle_hours: 5, overbooked_hours: 0 },
    clouds: [],
    ...overrides,
  };
}

export function rig(overrides: Partial<RigNights> = {}): RigNights {
  return {
    rig: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 },
    catalog_slug: 'redcat',
    catalog_name: 'RedCat 61',
    planning_site: { id: 'ssssssss-ssss-4sss-8sss-ssssssssssss', name: 'Backyard', revision: 1 },
    location: { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 },
    runtime: 'target_scheduler',
    profile_id: 'profile-a',
    plans: [{
      project_id: 7, guid: null, name: 'Heart Nebula', priority: 'normal',
      limits: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, horizon_offset_degrees: 0, custom_horizon: false, meridian_window_minutes: 0 },
      targets: [{ target_id: 70, name: 'IC 1805', position: { ra_degrees: 38.2, dec_degrees: 61.45 }, frames_owed: 60, hours_owed: 5.25, filters: ['Ha', 'OIII'] }],
    }],
    nights: [night(0), night(1, { availability: { state: 'unavailable', windows: [], marks: [{ scope: 'rig', state: 'unavailable', windows: [], note: 'dome motor', source: { kind: 'manual' } }] }, available_dark_hours: 0, targets: [{ project_id: 7, target_id: 70, hours_up: 6, hours_up_available: 0, hours_moon_blocked: 0, max_altitude_degrees: 71.2, transit_ms: null }], load: { demand_hours: 0, available_hours: 0, idle_hours: 0, overbooked_hours: 0 } }), night(2)],
    schedule: null,
    mark_revisions: { rig: 4, site: 0 },
    forecast: null,
    warnings: [],
    ...overrides,
  };
}

export function plans(rigs: RigNights[] = [rig()]): NightPlans {
  return {
    start_ms: NOON,
    nights: rigs[0]?.nights.length ?? 0,
    rigs,
    warnings: [],
    assumptions: { dark_below_degrees: -12, overhead_seconds_per_exposure: 15, sample_minutes: 5 },
  };
}
