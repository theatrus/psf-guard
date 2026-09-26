export interface DirectorStatus {
  protocol_version: number;
  enabled: boolean;
  instance_id: string | null;
  acquisition_available: boolean;
}

export type DirectorCollection = 'projects' | 'sites' | 'rigs';

export interface DirectorIdentity {
  id: string;
  name: string;
  revision: number;
}

export interface DirectorIdentityPage {
  items: DirectorIdentity[];
  next_after: string | null;
}
