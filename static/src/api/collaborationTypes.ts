export interface CollaborationBinding {
  id: string;
  rig_id: string;
  base_url: string;
  name: string;
  agent_id: string | null;
  allow_loopback_http: boolean;
  state: 'new' | 'outcome_unknown' | 'registered' | 'rejected' | 'disabled';
  settings?: CollaborationSettings;
}
export interface CollaborationConnection {
  binding: CollaborationBinding;
  status: 'not_connected' | 'registered' | 'disconnected' | 'credential_missing' | 'reauth_required' | 'storage_unavailable' | 'outcome_unknown' | 'awaiting_browser';
}
export interface CollaborationReply {
  status?: 'awaiting_browser';
  url?: string;
  expires_in?: number;
  signin?: boolean;
  pairing?: boolean;
}
export type CollaborationAction = 'discover' | 'pair' | 'signin' | 'poll' | 'cancel' | 'disconnect' | 'validate';
export interface CollaborationSettings {
  binning: number;
  colour: boolean;
  hours_per_night: number;
  share_status: boolean;
  filters: Record<string, { exposure_seconds: number; bandpass_nm: number | null }>;
}
export interface CollaborationNight { night: string; moon: number; moon_up: number }
export interface RemoteProject { project_id: string; name: string; joined: boolean; compatible: boolean | null }
export interface RemoteShare { task_id: string; name: string | null; version: number; review_reasons: string[]; demands: { panel_index: number; filter: string; exposure_ms: number; requested_frames: number }[] }
export interface CollaborationWork {
  projects?: RemoteProject[];
  shares?: RemoteShare[];
  preview?: { review_digest: string; action: string; acquisition_enabled: boolean };
  plan?: { project_id: string; import_id: string; night: string; share: RemoteShare };
  delivered?: number;
  accepted?: number;
  rejected?: number;
  catalogs?: { id: string; name: string }[];
  imports?: { id: string; name: string | null; night: string; panels: number[] }[];
  images?: { guid: string; filter: string; captured_at: number; target: string; file: string; panel?: number | null; source_digest?: string | null }[];
  review_digest?: string;
  report?: { frames: number; seconds: number; filterName: string; calibrated: boolean; footprint: { width: number; height: number } };
}
export interface ContributionSelection { import_id: string; catalog: string; panel: number; image_guids: string[]; source_digest?: string; observing_night?: string }
export interface CollaborationVisit {
  import_id: string; source_digest: string;
}
export interface CollaborationActivationContext {
  imports: { import_id: string; rig_id: string; rig_name: string; night: string; task_id: string; version: number; source_digest: string; demands: RemoteShare['demands'] }[];
}
export type CollaborationWorkInput =
  | { operation: 'report_inputs' }
  | { operation: 'report_candidates'; import_id: string; catalog: string; observing_night?: string }
  | { operation: 'preview_report'; selection: ContributionSelection }
  | { operation: 'queue_report'; selection: ContributionSelection; review_digest: string }
  | { operation: 'configure'; settings: CollaborationSettings }
  | { operation: 'browse' | 'checkin' }
  | { operation: 'tonight'; night: CollaborationNight }
  | { operation: 'join'; project: string; night: CollaborationNight }
  | { operation: 'preview'; task: string; night: CollaborationNight }
  | { operation: 'apply'; task: string; night: CollaborationNight; review_digest: string };
