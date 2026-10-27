export interface PolicyValues {
  analyze_new_frames: boolean;
  pair_calibrated_copies: boolean;
  scan_calibrated_copies: boolean;
}
export type PolicyOverrides = { [K in keyof PolicyValues]: boolean | null };
export interface DatabasePolicy {
  id: string; name: string; overrides: PolicyOverrides; effective: PolicyValues;
}
export interface ProcessingPolicies { defaults: PolicyValues; databases: DatabasePolicy[] }
