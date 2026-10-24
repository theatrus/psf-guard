import type { WbppStoragePolicy } from '../api/types';

export const DEFAULT_WBPP_STORAGE: WbppStoragePolicy = {
  max_volume_percent: 90,
  intermediate_days: 7,
  master_days: 90,
};

export function validWbppStorage(policy: WbppStoragePolicy): boolean {
  return (
    Number.isInteger(policy.max_volume_percent) &&
    policy.max_volume_percent >= 0 &&
    policy.max_volume_percent <= 100 &&
    Number.isInteger(policy.intermediate_days) &&
    policy.intermediate_days >= 1 &&
    policy.intermediate_days <= 36500 &&
    Number.isInteger(policy.master_days) &&
    (policy.master_days === 0 ||
      (policy.master_days >= policy.intermediate_days && policy.master_days <= 36500))
  );
}
