import { useCallback, useState, type MouseEvent } from 'react';
import { apiClient } from '../api/client';
import { useAccess } from '../auth/access';
import { openStudioLink, studioLink, useServerOrigin } from './studioLink';

/** Opening a target or a document in PSF Studio from this page: this
 *  server's address for the link, and a click handler that, on a server
 *  with sign-in, mints a one-time ticket and hands PSF Studio the link with
 *  it. */
export function useOpenInStudio(dbId: string, projectId: number) {
  const origin = useServerOrigin();
  const { status } = useAccess();
  const needsTicket = status.authentication_required;
  const [error, setError] = useState('');
  const open = useCallback(
    async (event: MouseEvent<HTMLAnchorElement>, targetId: number, documentId?: string) => {
      if (!needsTicket) return;
      event.preventDefault();
      setError('');
      try {
        const ticket = await apiClient.createStudioTicket();
        openStudioLink(studioLink(origin, dbId, projectId, targetId, ticket, documentId));
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : String(failure));
      }
    },
    [needsTicket, origin, dbId, projectId],
  );
  return { origin, error, open };
}
