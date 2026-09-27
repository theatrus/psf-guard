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

export interface DirectorCatalogIdentity { id: string; origin_instance_id: string }
export interface DirectorMapping {
  catalog_id: string;
  source_project_guid: string;
  source_profile_id: string;
  project_id: string;
  rig_id: string;
}
export interface DirectorDiscovery {
  catalog_slug: string;
  catalog_name: string;
  catalog_identity: DirectorCatalogIdentity | null;
  snapshot_digest: string;
  evidence: {
    projects: Array<{ source_row_id: number; source_project_guid: string | null; source_profile_id: string | null; name: string | null; issues: string[] }>;
    profiles: Array<{ source_profile_id: string; project_count: number }>;
  };
}
export interface DirectorMappingPage {
  catalog_identity: DirectorCatalogIdentity | null;
  items: DirectorMapping[];
  next_after: string | null;
}
export interface DirectorAdoptionPlan { catalog_id: string; mappings: DirectorMapping[] }
export interface DirectorAdoptionReport {
  catalog_identity: DirectorCatalogIdentity;
  preview_digest: string;
  applied: boolean;
  mappings: Array<{ mapping: DirectorMapping; source_name: string; project: DirectorIdentity; rig: DirectorIdentity }>;
}
