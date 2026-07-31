import { createContext, useContext } from 'react';
import type { AuthStatus } from '../api/types';

export interface AccessContextValue {
  status: AuthStatus;
  canWrite: boolean;
  canCompute: boolean;
  logout: () => Promise<void>;
}

const DEFAULT_ACCESS: AccessContextValue = {
  status: {
    authentication_required: false,
    authenticated: true,
    role: 'read_write',
    can_compute: true,
  },
  canWrite: true,
  canCompute: true,
  logout: async () => undefined,
};

export const AccessContext = createContext<AccessContextValue>(DEFAULT_ACCESS);

export function useAccess(): AccessContextValue {
  return useContext(AccessContext);
}
