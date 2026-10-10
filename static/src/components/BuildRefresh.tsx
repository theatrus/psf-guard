import { useEffect, useRef } from 'react';
import { useLocation } from 'react-router-dom';
import { reloadForNewerBuild, useNewerBuild, viewOf } from '../updates/pageBuild';

/** Once the server holds a newer build than this page, say so, and load it
 *  when the user next changes views. A page with unsaved edits holds that
 *  change of view until they are saved or dropped. */
export default function BuildRefresh() {
  const newer = useNewerBuild();
  const view = viewOf(useLocation().pathname);
  const shown = useRef(view);
  useEffect(() => {
    if (view === shown.current) return;
    shown.current = view;
    if (newer) reloadForNewerBuild();
  }, [view, newer]);

  if (!newer) return null;
  return (
    <aside className="update-notice" role="status">
      <span className="update-notice__copy">
        <strong>PSF Guard was updated</strong>
        <span>It reloads when you change views.</span>
      </span>
      <button type="button" onClick={() => window.location.reload()}>Reload now</button>
    </aside>
  );
}
