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

/** The link that has PSF Studio fetch one target's channel stacks from this
 *  server and open them. PSF Studio registers the `psf-studio` scheme. The
 *  link carries no token: PSF Studio asks the server as an anonymous caller. */
export function studioLink(origin: string, dbId: string, projectId: number, targetId: number): string {
  const query = new URLSearchParams({
    server: origin,
    db: dbId,
    project: String(projectId),
    target: String(targetId),
  });
  return `psf-studio://open?${query.toString()}`;
}
