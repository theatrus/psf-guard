import type { DirectorIdentity, DirectorSite, DirectorSkyPosition } from './directorTypes';

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

export interface NightAppliedMark {
  scope: NightMarkScope;
  state: NightAvailabilityState;
  windows: NightInterval[];
  note: string;
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
}

export type ScheduleGapReason = 'twilight' | 'unavailable' | 'moon' | 'meridian' | 'below_limits' | 'complete' | 'nothing_eligible';
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

export interface ScheduleRun {
  exposure_plan_id: number;
  filter: string;
  exposure_seconds: number;
  frames: number;
  dithers: number;
  start_ms: number;
  end_ms: number;
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
  warnings: string[];
}

export interface NightPlans {
  start_ms: number;
  nights: number;
  rigs: RigNights[];
  warnings: string[];
  assumptions: { dark_below_degrees: number; overhead_seconds_per_exposure: number; sample_minutes: number };
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
