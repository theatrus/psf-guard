export interface CollaborationBinding {
  id: string;
  rig_id: string;
  base_url: string;
  name: string;
  agent_id: string | null;
  allow_loopback_http: boolean;
  state: 'new' | 'outcome_unknown' | 'registered' | 'rejected' | 'disabled';
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
