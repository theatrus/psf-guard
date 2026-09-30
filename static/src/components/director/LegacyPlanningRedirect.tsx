import { Navigate, useSearchParams } from 'react-router-dom';
import { legacyPlanningHref } from './planAddress';

/** `/director`, the old Planning page: its links still land on their plan,
 *  their database row, or the Library. */
export default function LegacyPlanningRedirect() {
  const [params] = useSearchParams();
  return <Navigate to={legacyPlanningHref(params)} replace />;
}
