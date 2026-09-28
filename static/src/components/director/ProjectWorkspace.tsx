import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { ArrowLeft, ChevronDown, ChevronRight } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { ProjectPlanEditor } from '../ProjectSchedulerDialog';
import FramingView from './FramingView';
import PlanEditor from './PlanEditor';
import ActivationPanel from './ActivationPanel';
import type { FramingSeed } from './framingModel';
import { retryWhenBusy } from './retry';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

/** One global project: its linked source projects, framing, plan and activation. */
export default function ProjectWorkspace({ instanceId, projectId }: { instanceId: string; projectId: string }) {
  const [params] = useSearchParams();
  const { canWrite } = useAccess();
  const info = useQuery({ queryKey: ['serverInfo'], queryFn: apiClient.getServerInfo, staleTime: 300_000 });
  const status = useDirectorStatus();
  const manageable = status.data?.database_management ?? true;
  const plans = useQuery({ queryKey: ['directorPlans', instanceId], queryFn: apiClient.getDirectorPlans, retry: retryWhenBusy, retryDelay: 1200, refetchOnWindowFocus: false, refetchOnMount: 'always' });
  const row = plans.data?.rows.find(entry => entry.project.id === projectId);
  const first = row?.links.find(link => link.source_row_id !== null);
  // The first linked catalog project seeds the framing; TS keeps RA in hours.
  const scheduler = useQuery({
    queryKey: ['db', first?.catalog_slug, 'project-scheduler', first?.source_row_id],
    queryFn: () => apiClient.getProjectScheduler(first!.catalog_slug, first!.source_row_id!),
    enabled: !!first, retry: false,
  });
  const seed: FramingSeed | null = useMemo(() => {
    const target = scheduler.data?.targets[0];
    if (!target) return null;
    return {
      name: target.name,
      center: { ra_degrees: ((target.ra_hours * 15) % 360 + 360) % 360, dec_degrees: target.dec_degrees },
      position_angle_degrees: Number.isFinite(target.rotation) ? ((target.rotation % 360) + 360) % 360 : 0,
    };
  }, [scheduler.data]);
  // The Library's Planning button names the database it came from; that
  // database's targets and exposures open at once.
  const cameFrom = params.get('db');
  const [openSource, setOpenSource] = useState<string | null | undefined>(undefined);
  const arrival = row?.links.find(link => link.catalog_slug === cameFrom && link.source_row_id !== null);
  const openKey = openSource === undefined
    ? arrival ? `${arrival.catalog_slug}:${arrival.source_project_guid}` : null
    : openSource;
  const back = new URLSearchParams(params);
  back.delete('directorProject');
  if (plans.isPending) return <p role="status">Loading project...</p>;
  if (plans.isError) return <div role="alert"><p>{message(plans.error)}</p><button type="button" onClick={() => void plans.refetch()}>Retry</button></div>;
  if (!row) return <p role="alert">Project not found. <Link to={`/director?${back}`}>Back to plans</Link></p>;
  return <section aria-label="Project planning" className="director-workspace">
    <div className="director-toolbar director-workspace-head"><Link to={`/director?${back}`}><ArrowLeft size={16} />Plans</Link><h2>{row.project.name}</h2></div>
    <h3 className="director-section-heading director-framing-heading">Framing</h3>
    {first && scheduler.isPending ? <p role="status">Loading targets...</p> : <FramingView projectId={projectId} seed={seed} preferredRigIds={row.links.map(link => link.rig.id)} />}
    <div className="director-workspace-columns">
      <div><h3 className="director-section-heading">Plan</h3><PlanEditor projectId={projectId} /></div>
      <div><h3 className="director-section-heading">Activation</h3><ActivationPanel projectId={projectId} /></div>
    </div>
    <section aria-label="Linked databases">
      <h3 className="director-section-heading">Databases</h3>
      {!manageable && <p className="director-muted">This server cannot change rig databases, so the targets and exposures below are view only.</p>}
      {row.links.length === 0 && <p className="director-muted">No database holds this project yet. Activation creates it in each rig you tick in the plan.</p>}
      <ul className="director-list">
        {row.links.map(link => {
          const key = `${link.catalog_slug}:${link.source_project_guid}`;
          const open = openKey === key;
          return <li key={key}>
            <div className="director-record director-record-wide">
              <div className="director-record-name">
                <strong>{link.catalog_name}</strong>
                <span className="director-muted">{link.source_name ?? 'Project row missing in this database'}</span>
              </div>
              {link.source_row_id !== null && <button type="button" aria-expanded={open} onClick={() => setOpenSource(open ? null : key)}>{open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}Targets and exposures</button>}
            </div>
            {open && link.source_row_id !== null && <ProjectPlanEditor dbId={link.catalog_slug} projectId={link.source_row_id} canEdit={canWrite && !!info.data?.allow_database_management} />}
          </li>;
        })}
      </ul>
    </section>
  </section>;
}
