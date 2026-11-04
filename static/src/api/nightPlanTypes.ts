import type { DirectorIdentity, DirectorSite, DirectorSkyPosition, ForecastHour, ForecastProvider } from './directorTypes';

/** Which runtime picks a rig's work on the night. */
export type NightRuntime = 'target_scheduler' | 'director';
export type NightAvailabilityState = 'available' | 'unavailable' | 'limited';
export type NightMarkScope = 'site' | 'rig';
export type NightPlanPriority = 'low' | 'normal' | 'high';

/** Half-open, in Unix milliseconds. */
export interface NightInterval {
  start_ms: number;
  end_ms: number;
}

export interface NightPlanLimits {
  minimum_altitude_degrees: number;
  maximum_altitude_degrees: number;
  horizon_offset_degrees: number;
  custom_horizon: boolean;
  meridian_window_minutes: number;
}

export interface NightPlanTarget {
  target_id: number;
  name: string;
  position: DirectorSkyPosition;
  frames_owed: number;
  /** Exposure and overhead time for the frames owed. */
  hours_owed: number;
  filters: string[];
}

/** A Target Scheduler project the rig still owes frames for. */
export interface NightPlanProject {
  project_id: number;
  guid: string | null;
  name: string;
  priority: NightPlanPriority;
  limits: NightPlanLimits;
  targets: NightPlanTarget[];
}

/** Where a mark came from: by hand, or the site's forecast where no mark
 *  says otherwise. */
export type NightMarkSource =
  | { kind: 'manual' }
  | { kind: 'forecast'; provider: ForecastProvider; fetched_at_ms: number };

export interface NightAppliedMark {
  scope: NightMarkScope;
  state: NightAvailabilityState;
  windows: NightInterval[];
  note: string;
  source: NightMarkSource;
}

export interface NightAvailabilityView {
  state: NightAvailabilityState;
  /** A limited night's clear hours, the site's and the rig's met. */
  windows: NightInterval[];
  marks: NightAppliedMark[];
}

export interface NightTargetHours {
  project_id: number;
  target_id: number;
  hours_up: number;
  hours_up_available: number;
  hours_moon_blocked: number;
  max_altitude_degrees: number;
  transit_ms: number | null;
}

export interface NightLoad {
  demand_hours: number;
  available_hours: number;
  idle_hours: number;
  overbooked_hours: number;
}

export interface RigNight {
  /** The evening's local date, YYYY-MM-DD. */
  date: string;
  noon_ms: number;
  dusk_ms: number | null;
  dawn_ms: number | null;
  dark_hours: number;
  available_dark_hours: number;
  availability: NightAvailabilityView;
  moon_illumination: number;
  moon_hours_up_in_dark: number;
  targets: NightTargetHours[];
  load: NightLoad;
  /** The forecast's cloud cover through the night; empty without one. */
  clouds: ForecastHour[];
  /** How many overrides a Director rig has for the night; 0 on any other rig. */
  override_count: number;
}

export type ScheduleGapReason = 'twilight' | 'unavailable' | 'moon' | 'meridian' | 'below_limits' | 'complete' | 'nothing_eligible' | 'held' | 'excluded';
export type ScheduleDecisionKind = 'only' | 'scored' | 'continued' | 'ranked';
export type ScheduleRule = 'meridian_flip_penalty' | 'meridian_window_priority' | 'mosaic_completion' | 'percent_complete' | 'project_priority' | 'setting_soonest' | 'smart_exposure_order' | 'target_switch_penalty';
export type ScheduleTargetReason = 'complete' | 'never_rises' | 'not_visible' | 'not_yet_visible' | 'max_altitude' | 'meridian_window_clipped' | 'before_meridian_window' | 'meridian_flip_clipped' | 'twilight' | 'moon_avoidance' | 'lower_score' | 'all_exposure_plans' | 'project' | 'not_ready';
export type ScheduleFidelity = 'estimate' | 'target_scheduler_preview';
export type ScheduleRuntimeChoice = 'auto' | NightRuntime;

export interface ScheduleTargetRef {
  project_id: number;
  project_name: string;
  target_id: number;
  target_name: string;
}

/** The Moon over a filter run, sampled at its ends and every quarter hour. */
export interface RunMoon {
  /** The Moon's highest altitude; negative while it stays down. */
  max_altitude_degrees: number;
  /** The closest the Moon comes to the run's target. */
  min_separation_degrees: number;
}

export interface ScheduleRun {
  exposure_plan_id: number;
  filter: string;
  exposure_seconds: number;
  frames: number;
  dithers: number;
  start_ms: number;
  end_ms: number;
  /** Absent when the target has no position. */
  moon?: RunMoon;
}

export interface ScheduleCandidate {
  target: ScheduleTargetRef;
  /** null for the chosen target. */
  rejected: ScheduleTargetReason | null;
  score: { total: number; rules: Array<{ rule: ScheduleRule; raw: number; weight: number }> } | null;
  could_start_ms: number | null;
}

export interface ScheduleBlock {
  target: ScheduleTargetRef;
  start_ms: number;
  end_ms: number;
  slew: boolean;
  runs: ScheduleRun[];
  decision: { kind: ScheduleDecisionKind; candidates: ScheduleCandidate[]; note?: string };
}

export interface ScheduleGap {
  start_ms: number;
  end_ms: number;
  reason: ScheduleGapReason;
  waiting_for: ScheduleTargetRef | null;
}

export interface ScheduledNight {
  date: string;
  start_ms: number;
  end_ms: number;
  blocks: ScheduleBlock[];
  gaps: ScheduleGap[];
  frames: number;
  exposure_hours: number;
  note: string | null;
  /** The Director rig's overrides the night was planned with. */
  overrides?: AppliedNightOverrides;
}

/** What an override applies to. */
export type OverrideSubject =
  | { kind: 'plan'; plan_id: string }
  | { kind: 'target'; target_guid: string }
  | { kind: 'exposure_plan'; exposureplan_guid: string };

/** One override of a Director rig's night. */
export type NightOverride =
  | { kind: 'exclude'; subject: OverrideSubject; window?: NightInterval }
  | { kind: 'pin'; subject: OverrideSubject; window: NightInterval }
  | { kind: 'order'; plans: string[] }
  | { kind: 'hold'; from_ms?: number };

export interface AppliedNightOverrides {
  /** The stored list's revision; null for an unsaved draft. */
  revision: number | null;
  entries: NightOverride[];
  draft: boolean;
}

/** One night's overrides, as the editor starts from them. */
export interface NightOverridesNight {
  date: string;
  /** Send back with a change: 0 while the night has none. */
  revision: number;
  entries: NightOverride[];
  runtime: NightRuntime;
}

/** A goal of a Director schedule: the ids its blocks use, and the GUIDs an
 *  override names it by. */
export interface DirectorScheduleGoal {
  goal_id: string;
  plan_id: string;
  plan_name: string;
  target_guid: string;
  exposureplan_guid: string;
  project_id: number;
  target_id: number;
  exposure_plan_id: number;
  target_name: string;
  filter: string | null;
}

export interface SchedulePlanProgress {
  project_id: number;
  target_id: number;
  exposure_plan_id: number;
  filter: string;
  desired: number;
  accepted_before: number;
  acquired_before: number;
  /** Frames planned on each scheduled night. */
  planned: number[];
  /** Index of the night after which the plan owes nothing. */
  completes_on: number | null;
  complete_before: boolean;
}

export interface NightSchedule {
  runtime: NightRuntime;
  nights: ScheduledNight[];
  progress: SchedulePlanProgress[];
  assumptions: {
    target_scheduler_version: string | null;
    fidelity: ScheduleFidelity;
    overheads: { per_exposure_seconds: number; target_change_seconds: number; filter_change_seconds: number; dither_seconds: number } | null;
    acceptance: 'plan_rate' | 'grader';
    utc_offset_minutes: number;
  };
  /** A Director schedule's goals; absent for Target Scheduler. */
  goals?: DirectorScheduleGoal[];
}

export interface RigNights {
  rig: DirectorIdentity;
  catalog_slug: string;
  catalog_name: string;
  planning_site: DirectorIdentity | null;
  location: DirectorSite;
  runtime: NightRuntime;
  profile_id: string | null;
  plans: NightPlanProject[];
  nights: RigNight[];
  /** What the rig would shoot, when schedules were asked for. */
  schedule: NightSchedule | null;
  mark_revisions: { rig: number; site: number | null };
  /** The cloud forecast the rig's planning site takes. */
  forecast: RigForecast | null;
  /** The planning site's IANA time zone; null uses the browser's. */
  time_zone: string | null;
  /** Every project of the planned profile, in any state, as the rig's
   *  database holds it; absent without a database to read. */
  ts_projects?: TsProject[];
  /** Where saving Target Scheduler changes writes, or why it cannot. */
  ts_save: { files: string[]; refused: string | null };
  warnings: string[];
}

export type TsProjectState = 'draft' | 'active' | 'inactive' | 'closed';

/** A Target Scheduler project, as the night's controls show it. */
export interface TsProject {
  project_id: number;
  name: string;
  state: TsProjectState;
  priority: NightPlanPriority;
  targets: Array<{ target_id: number; name: string; active: boolean }>;
}

/** Changes to a rig's Target Scheduler rows: with `was` and `was_active`
 *  to save, without them to preview. */
export interface TsChanges {
  projects: Array<{ project_id: number; priority?: NightPlanPriority; state?: TsProjectState; was?: { priority: NightPlanPriority; state: TsProjectState } }>;
  targets: Array<{ target_id: number; active: boolean; was_active?: boolean }>;
}

export interface RigForecast {
  provider: ForecastProvider;
  /** The credit the provider's licence asks for. */
  attribution: string;
  cloud_threshold_percent: number;
  /** The forecast planning uses; null when none is kept or it expired. */
  fetched_at_ms: number | null;
  expires_at_ms: number | null;
}

export interface NightPlans {
  start_ms: number;
  nights: number;
  rigs: RigNights[];
  warnings: string[];
  assumptions: { dark_below_degrees: number; overhead_seconds_per_exposure: number; sample_minutes: number };
  /** When the server computed these plans; it keeps them, and computes them
   *  again when what they come from changes. */
  computed_at_ms: number;
  /** Newer plans are being computed; read again shortly. */
  refreshing: boolean;
}

export interface NightPlansRequest {
  rigs?: string[];
  start_ms?: number;
  nights?: number;
  /** Schedule this many nights from the first. */
  schedule_nights?: number;
  /** The scheduled night whose blocks carry the planner's reasons. */
  explain?: string;
  runtime?: ScheduleRuntimeChoice;
  fidelity?: ScheduleFidelity;
  /** The site's clock, minutes east of UTC. */
  utc_offset_minutes?: number;
  /** The first night, the evening's date at each rig's site. */
  first_night?: string;
  /** Schedules plan nothing before this instant; now by default. */
  plan_from_ms?: number;
  /** An unsaved edit of one Director rig's overrides for one night. */
  override_draft?: { rig: string; date: string; entries: NightOverride[] };
  /** Unsaved changes to one Target Scheduler rig's projects and targets. */
  ts_draft?: { rig: string; changes: TsChanges };
  /** Compute the plans again, whatever the server keeps. */
  refresh?: boolean;
}

/** One night to mark, as a person enters it. */
export interface NightMarkEdit {
  date: string;
  state: NightAvailabilityState;
  windows?: NightInterval[];
  note?: string;
}

export interface NightMark {
  date: string;
  state: NightAvailabilityState;
  windows: NightInterval[];
  note: string;
  revision: number;
  updated_at_ms: number;
}

export interface NightMarks {
  scope: NightMarkScope;
  scope_id: string;
  revision: number;
  marks: NightMark[];
}
