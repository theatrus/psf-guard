import { useEffect, useRef } from 'react';
import { Link, Navigate, useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import { withoutPlanningParams } from '../../hooks/useUrlState';
import { usePlans } from '../header/useCurrentPlan';
import './DirectorPage.css';
import DirectorProjectContext from './DirectorProjectContext';
import ProjectWorkspace from './ProjectWorkspace';
import { samePageAs } from './pageDraftsState';
import { planHref, planKey, resolvePlan } from './planAddress';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

/** One plan's workspace at `/plan?plan=<key>`. The key is the Target
 *  Scheduler GUID its rigs share, the plan id, or `slug:row` for a project
 *  not planned yet (see planAddress). */
export default function PlanPage() {
  const [params] = useSearchParams();
  const { search } = useLocation();
  const navigate = useNavigate();
  const status = useDirectorStatus();
  const plans = usePlans();
  // The plan this page showed last: when a detach leaves two plans holding
  // the address's GUID, the one already open stays open.
  const shown = useRef<string | null>(null);
  const available = status.data?.enabled && status.data.protocol_version === 1 && !!status.data.instance_id;
  const key = params.get('plan');
  const resolved = resolvePlan(plans.rows, key, shown.current);
  if (resolved.kind === 'plan') shown.current = resolved.row.project.id;
  // Keep the address current: a row, an old id or a GUID another plan now
  // shares becomes the plan's own key, so bookmarks and history stay right.
  // The workspace stays mounted meanwhile: a detach, an attach or a first
  // activation changes the key of the plan already open, and its unsaved
  // edits, notices and open dialog must survive that.
  const canonical = resolved.kind === 'plan' ? planKey(resolved.row, plans.rows) : null;
  useEffect(() => {
    if (canonical && key !== canonical) navigate(planHref(canonical, params), { replace: true, state: samePageAs(search) });
  }, [canonical, key, params, search, navigate]);
  // A plan opens at its top, not at the scroll offset of the list it came
  // from; a new key for the plan already open keeps the place.
  const opened = resolved.kind === 'plan' ? resolved.row.project.id : key;
  useEffect(() => { document.querySelector('.app-main')?.scrollTo?.({ top: 0 }); }, [opened]);
  const back = withoutPlanningParams(params.toString()).toString();
  const library = <Link to={back ? `/?${back}` : '/'}>Library</Link>;
  return (
    <main className="director-page">
      {/* No page title: the header's Planning tab and the plan's summary say
          where you are. */}
      {status.isPending && <p role="status">Loading plans...</p>}
      {status.isError && <div role="alert"><p>{message(status.error)}</p><button type="button" onClick={() => void status.refetch()}>Retry</button></div>}
      {status.data && !available && <p>Plans are unavailable on this server.</p>}
      {available && status.data && <>
        {/* The workspace says so in its summary, inside the sticky top it measures. */}
        {!status.data.database_management && resolved.kind !== 'plan' && <p className="director-muted" role="note">Drafts only: this server does not write to rig databases.</p>}
        {!key
          ? <Navigate to={back ? `/?${back}` : '/'} replace />
          : resolved.kind === 'plan'
          ? <ProjectWorkspace key={`${status.data.instance_id}:${resolved.row.project.id}`} instanceId={status.data.instance_id!} projectId={resolved.row.project.id} />
          : resolved.kind === 'ambiguous'
          ? <div role="alert"><p>Several plans carry this Target Scheduler GUID; pick one.</p>
              <ul>{resolved.rows.map(row => <li key={row.project.id}><Link to={planHref(row.project.id, params)}>{row.project.name}</Link> <span className="director-muted">{row.links.map(link => link.catalog_name).join(', ')}</span></li>)}</ul></div>
          : resolved.kind === 'source'
          ? <DirectorProjectContext key={`${status.data.instance_id}:${resolved.slug}:${resolved.projectId}`} instanceId={status.data.instance_id!} slug={resolved.slug} projectId={resolved.projectId} />
          : plans.loading
          ? <p role="status">Finding the plan...</p>
          : plans.error
          ? <div role="alert"><p>{message(plans.query.error)}</p><button type="button" onClick={() => void plans.query.refetch()}>Retry</button></div>
          : <p role="alert">No plan goes by that name here. {library}</p>}
      </>}
    </main>
  );
}
