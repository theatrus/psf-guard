import { isAxiosError } from 'axios';
import type { CollaborationConnection } from '../../api/collaborationTypes';

export const collaborationMessage = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || 'Collaboration request failed'
  : error instanceof Error ? error.message : 'Collaboration request failed';

export const connectionStates: Record<CollaborationConnection['status'], string> = {
  not_connected: 'Not connected', registered: 'Connected', disconnected: 'Disconnected',
  credential_missing: 'Credential missing', reauth_required: 'Credential rejected',
  storage_unavailable: 'Credential file unavailable', outcome_unknown: 'Registration outcome unknown',
  awaiting_browser: 'Waiting for browser approval',
};
