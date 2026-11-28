import { useEffect, useState } from 'react';
import { getServerUrl, isTauriApp } from './tauri';

/** This server's address, as a program on the same computer as the browser
 *  reaches it. Empty until the desktop app has reported its own. */
export function useServerOrigin(): string {
  const [origin, setOrigin] = useState(() => (isTauriApp() ? '' : window.location.origin));
  useEffect(() => {
    if (!isTauriApp()) return;
    let live = true;
    void getServerUrl()
      .then((url) => { if (live) setOrigin(url); })
      .catch(() => { if (live) setOrigin(''); });
    return () => { live = false; };
  }, []);
  return origin;
}

/** The link that has PSF Studio open one target from this server: its
 *  latest Studio document, or a new one from the target's channel stacks.
 *  PSF Studio registers the `psf-studio` scheme. On a server with sign-in,
 *  `ticket` is a one-time ticket from `/api/auth/studio/ticket`, which PSF
 *  Studio trades for a connection when it has none to this server yet. The
 *  link never carries a lasting token. */
export function studioLink(
  origin: string,
  dbId: string,
  projectId: number,
  targetId: number,
  ticket?: string,
): string {
  const query = new URLSearchParams({
    server: origin,
    db: dbId,
    project: String(projectId),
    target: String(targetId),
  });
  if (ticket) query.set('ticket', ticket);
  return `psf-studio://open?${query.toString()}`;
}

/** Hands a link to PSF Studio through the system. */
export function openStudioLink(link: string): void {
  window.location.href = link;
}
