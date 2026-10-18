import { useQuery } from '@tanstack/react-query';
import { Link, Navigate, useSearchParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { apiClient } from '../../api/client';
import { retryWhenBusy } from './retry';
import { withoutPlanningParams } from '../../hooks/useUrlState';
import { planHref, planKey } from './planAddress';

/** A database's project row that has no plan yet (`/plan?plan=slug:row`).
 *  Listing the plans adopts the database, so the row usually finds its plan
 *  here and moves on. Copies, unreadable catalogs and rows without GUIDs
 *  can all be left out; show the actual warning rather than guessing. */
export default function DirectorProjectContext({ instanceId, slug, projectId }: {
  instanceId: string; slug: string; projectId: number;
}) {
  const [params] = useSearchParams();
  const plans = useQuery({ queryKey: ['directorPlans', instanceId], queryFn: apiClient.getDirectorPlans, retry: retryWhenBusy, retryDelay: 700, refetchOnMount: 'always' });
  const back = withoutPlanningParams(params.toString());
  // Listing the plans takes the database in; once its row has a plan, go there.
  const row = plans.data?.rows.find(entry => entry.links.some(link => link.catalog_slug === slug && link.source_row_id === projectId));
  const projects = useQuery({ queryKey: ['db', slug, 'projects'], queryFn: () => apiClient.getProjects(slug), enabled: plans.isSuccess && !row, retry: false });
  const project = projects.data?.find(entry => entry.id === projectId);
  if (row) return <Navigate to={planHref(planKey(row, plans.data!.rows), params)} replace />;
  return <section aria-label="Project acquisition planning">
    <div className="director-toolbar"><Link to={`/?${back}`}><ArrowLeft size={16} />Library</Link></div>
    {plans.isPending && <p role="status">Finding this project's plan...</p>}
    {plans.isError && <div role="alert"><p>{plans.error.message}</p><button type="button" onClick={() => void plans.refetch()}>Retry</button></div>}
    {plans.data && <>
      <p role="alert">This project is not available for planning. Review the catalog warnings below, then retry.</p>
      {project && !project.guid?.trim() && <p>This project has no Target Scheduler GUID. Settings › Databases offers <strong>Fill in GUIDs</strong> for that database.</p>}
      {projects.isSuccess && !project && <p>This project is no longer in the catalog.</p>}
      {projects.isError && <p role="alert">Could not check this project: {projects.error.message}</p>}
      <button type="button" disabled={plans.isFetching || projects.isFetching} onClick={() => { void plans.refetch(); void projects.refetch(); }}>Retry planning</button>
    </>}
    {plans.data?.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
  </section>;
}
