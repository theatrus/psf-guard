import { useQuery } from '@tanstack/react-query';
import { Link, Navigate, useSearchParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { apiClient } from '../../api/client';
import { retryWhenBusy } from './retry';

/** The Library's "Open in Planning" arrives here with a database and project row;
 *  the plan list knows which plan that row belongs to. */
export default function DirectorProjectContext({ instanceId, slug, projectId }: {
  instanceId: string; slug: string; projectId: number;
}) {
  const [params] = useSearchParams();
  const plans = useQuery({ queryKey: ['directorPlans', instanceId], queryFn: apiClient.getDirectorPlans, retry: retryWhenBusy, retryDelay: 700, refetchOnMount: 'always' });
  const back = new URLSearchParams(params);
  back.delete('directorView'); back.delete('directorSource'); back.delete('directorCatalog');
  const row = plans.data?.rows.find(entry => entry.links.some(link => link.catalog_slug === slug && link.source_row_id === projectId));
  if (row) {
    const next = new URLSearchParams(params);
    next.delete('directorSource'); next.delete('directorView'); next.delete('directorCatalog');
    next.set('directorProject', row.project.id);
    return <Navigate to={`/director?${next}`} replace />;
  }
  return <section aria-label="Project acquisition planning">
    <div className="director-toolbar"><Link to={`/?${back}`}><ArrowLeft size={16} />Library</Link></div>
    {plans.isPending && <p role="status">Finding this project's plan...</p>}
    {plans.isError && <div role="alert"><p>{plans.error.message}</p><button type="button" onClick={() => void plans.refetch()}>Retry</button></div>}
    {plans.data && <p role="alert">This project has no plan yet. A Target Scheduler project needs a GUID to be planned; open it in Target Scheduler once, or import a frame into it.</p>}
    {plans.data?.warnings.map(warning => <p key={warning} className="director-error" role="alert">{warning}</p>)}
  </section>;
}
