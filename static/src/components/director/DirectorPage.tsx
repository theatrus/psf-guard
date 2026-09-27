import { useEffect } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import './DirectorPage.css';
import DirectorProjectContext from './DirectorProjectContext';
import DirectorPlans from './DirectorPlans';
import DirectorRigs from './DirectorRigs';
import { openSettings } from '../../utils/settingsIntent';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

export default function DirectorPage() {
  const status = useDirectorStatus();
  const [params, setParams] = useSearchParams();
  const selected = params.get('directorView');
  const available = status.data?.enabled && status.data.protocol_version === 1 && !!status.data.instance_id;
  const sourceSlug = params.get('directorSource');
  const rawProject = params.get('project') ?? '';
  const projectId = /^\d+$/.test(rawProject) && Number.isSafeInteger(Number(rawProject)) ? Number(rawProject) : null;
  // Older links: the Catalogs tab now lives under the database's settings, and
  // the Sites and Rigs tabs are sections of this page.
  useEffect(() => {
    if (!available || !selected) return;
    if (selected === 'catalogs') {
      const slug = params.get('directorCatalog');
      if (slug) openSettings({ kind: 'director-links', dbId: slug });
      else openSettings();
    }
    if (selected !== 'projects') {
      const next = new URLSearchParams(params);
      next.set('directorView', 'projects'); next.delete('directorCatalog');
      setParams(next, { replace: true });
    }
  }, [selected, available, params, setParams]);
  return (
    <main className="director-page">
      <header className="director-heading"><h1>Director</h1><span className="director-preview">Experimental</span></header>
      {status.isPending && <p role="status">Loading Director...</p>}
      {status.isError && <div role="alert"><p>{message(status.error)}</p><button type="button" onClick={() => void status.refetch()}>Retry</button></div>}
      {status.data && !available && <p>Director management is unavailable on this server.</p>}
      {available && status.data && <>
        {!status.data.acquisition_available && <p className="director-muted">Acquisition is not yet available.</p>}
        {sourceSlug
          ? projectId !== null && sourceSlug === params.get('db')
            ? <DirectorProjectContext key={`${status.data.instance_id}:${sourceSlug}:${projectId}`} instanceId={status.data.instance_id!} slug={sourceSlug} projectId={projectId} />
            : <p role="alert">Invalid project scope. <Link to="/">Overview</Link></p>
          : <>
            <Link to="/">Overview projects</Link>
            <DirectorPlans key={status.data.instance_id!} instanceId={status.data.instance_id!} />
            <DirectorRigs />
          </>}
      </>}
    </main>
  );
}
