import { useQuery } from '@tanstack/react-query';
import { Link, Navigate, useSearchParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { apiClient } from '../../api/client';
import { retryWhenBusy } from './retry';
import { withoutPlanningParams } from '../../hooks/useUrlState';
import { planHref, planKey } from './planAddress';

/** A database's project row that has no plan yet (`/plan?plan=slug:row`).
 *  Listing the plans adopts the database, so the row usually finds its plan
 *  here and moves on; a row without a GUID cannot be planned and says so. */
export default function DirectorProjectContext({ instanceId, slug, projectId }: {
  instanceId: string; slug: string; projectId: number;
}) {
  const [params] = useSearchParams();
  const plans = useQuery({ queryKey: ['directorPlans', instanceId], queryFn: apiClient.getDirectorPlans, retry: retryWhenBusy, retryDelay: 700, refetchOnMount: 'always' });
  const back = withoutPlanningParams(params.toString());
  // Listing the plans takes the database in; once its row has a plan, go there.
  const row = plans.data?.rows.find(entry => entry.links.some(link => link.catalog_slug === slug && link.source_row_id === projectId));
  if (row) return <Navigate to={planHref(planKey(row, plans.data!.rows), params)} replace />;
  return <section aria-label="Project acquisition planning">
    <div className="director-toolbar"><Link to={`/?${back}`}><ArrowLeft size={16} />Library</Link></div>
    {plans.isPending && <p role="status">Finding this project's plan...</p>}
    {plans.isError && <div role="alert"><p>{plans.error.message}</p><button type="button" onClick={() => void plans.refetch()}>Retry</button></div>}
    {plans.data && <p role="alert">This project has no plan yet. A Target Scheduler project needs a GUID to be planned. If its database was upgraded past Target Scheduler schema 22 in one step, its older rows never got one: Settings › Databases offers <strong>Fill in GUIDs</strong> for that database.</p>}
    {plans.data?.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
  </section>;
}
