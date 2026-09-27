import { useQuery } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { ArrowLeft, Settings } from 'lucide-react';
import { loadCatalog } from './catalogData';
import { openSettings } from '../../utils/settingsIntent';
import { ProjectPlanEditor } from '../ProjectSchedulerDialog';
import { useAccess } from '../../auth/access';
import { apiClient } from '../../api/client';
import FramingView from './FramingView';
import type { FramingSeed } from './framingModel';

export default function DirectorProjectContext({ instanceId, slug, projectId }: {
  instanceId: string; slug: string; projectId: number;
}) {
  const [params] = useSearchParams();
  const { canWrite } = useAccess();
  const info = useQuery({ queryKey: ['serverInfo'], queryFn: apiClient.getServerInfo, staleTime: 300_000 });
  const query = useQuery({ queryKey: ['directorCatalog', instanceId, slug], queryFn: () => loadCatalog(slug), retry: false });
  const scheduler = useQuery({ queryKey: ['db', slug, 'project-scheduler', projectId], queryFn: () => apiClient.getProjectScheduler(slug, projectId), retry: false });
  const data = query.data;
  const source = data?.discovery.evidence.projects.find(project => project.source_row_id === projectId);
  const mapping = source?.issues.length === 0 && data?.mappings.find(item =>
    item.source_project_guid === source.source_project_guid && item.source_profile_id === source.source_profile_id);
  const project = mapping && data?.projects.find(item => item.id === mapping.project_id);
  const rig = data?.rig;
  // The first catalog target seeds a framing draft: TS keeps RA in hours.
  const seedTarget = scheduler.data?.targets[0];
  const seed: FramingSeed | null = seedTarget ? {
    name: seedTarget.name,
    center: { ra_degrees: ((seedTarget.ra_hours * 15) % 360 + 360) % 360, dec_degrees: seedTarget.dec_degrees },
    position_angle_degrees: Number.isFinite(seedTarget.rotation) ? ((seedTarget.rotation % 360) + 360) % 360 : 0,
  } : null;
  const back = new URLSearchParams(params);
  back.delete('directorView'); back.delete('directorSource'); back.delete('directorCatalog');
  return <section aria-label="Project acquisition planning">
    <div className="director-toolbar"><Link to={`/?${back}`}><ArrowLeft size={16} />Overview</Link>
      <button type="button" onClick={() => openSettings({ kind: 'director-links', dbId: slug })}><Settings size={16} />Project planning links</button>
    </div>
    {query.isPending && <p role="status">Loading project...</p>}
    {query.isError && <div role="alert"><p>{query.error.message}</p><button type="button" onClick={() => void query.refetch()}>Retry</button></div>}
    {data && !query.isError && (source ? <>
      <h2>{project ? project.name : source.name ?? `Project ${projectId}`}</h2>
      <dl className="director-project-context">
        <dt>Catalog project</dt><dd>{source.name ?? `Project ${projectId}`}</dd>
        <dt>Database</dt><dd>{data.discovery.catalog_name}</dd>
        <dt>Rig</dt><dd>{data.discovery.catalog_name}</dd>
        <dt>Rig setup</dt><dd>{rig ? 'Database-backed' : 'Planning not enabled'}</dd>
        <dt>Planning link</dt><dd>{mapping ? 'Linked' : source.issues.length ? 'Source identity needs attention' : 'Not linked'}</dd>
      </dl>
      <ProjectPlanEditor dbId={slug} projectId={projectId} canEdit={canWrite && !!info.data?.allow_database_management} />
      <h3 className="director-section-heading">Framing</h3>
      {project ? (scheduler.isPending ? <p role="status">Loading targets...</p> : <FramingView projectId={project.id} seed={seed} />)
        : <p className="director-muted">Link this project under Project planning links to frame it across rigs.</p>}
    </> : <p role="alert">Project no longer exists in this database.</p>)}
  </section>;
}
