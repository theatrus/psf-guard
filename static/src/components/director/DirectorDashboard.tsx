import { useEffect } from 'react';
import { Link, useLocation, useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { RefreshCw } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorRigStatusView, ForecastProvider } from '../../api/directorTypes';
import type { RigNights } from '../../api/nightPlanTypes';
import { describeNow, errorsOf, formatAge } from './dashboardModel';
import { describeRecent, describeSequence, describeSyncAction } from './syncLiveModel';
import { retryWhenBusy } from './retry';
import EarlyBeta from './EarlyBeta';
import { RigWeatherSummary } from './RigWeather';
import { PROVIDER_URL } from './forecastLabels';
import { planHref } from './planAddress';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import { formatHours } from './visibilityFormat';
import { nightHref, plansScheduled, refetchKept } from '../nights/nightsModel';

const message = (error: unknown) => error instanceof Error ? error.message : 'Director request failed';

const STATE_LABEL: Record<DirectorRigStatusView['connectivity']['state'], string> = { online: 'Online', stale: 'Quiet', offline: 'Offline', never: 'Never seen' };

/** Every rig as the operator wants to see it at a glance: reachable or not,
 *  what it is doing, when it last pulled, checked in and reported, and what
 *  it is assigned. Ages are from server receipt times; a stale report is
 *  labelled, never shown as the present. */
export default function DirectorDashboard() {
  const [params] = useSearchParams();
  const statuses = useQuery({ queryKey: ['directorRigStatuses'], queryFn: apiClient.getDirectorRigStatuses, retry: retryWhenBusy, retryDelay: 1200, refetchInterval: 15_000, refetchOnWindowFocus: true });
  const now = useDirectorClock();
  // Listing the plans gives each new database its rig, as Rig setup does, so
  // a database only the Sync plugin uses appears here before anyone opens
  // Planning. Read the rigs again once it has.
  const bound = useQuery({ queryKey: ['liveRigBinding'], queryFn: async () => { await apiClient.getDirectorPlans(); return Date.now(); }, retry: false, staleTime: 5 * 60_000, refetchOnWindowFocus: false });
  const refetchStatuses = statuses.refetch;
  useEffect(() => { if (bound.data) void refetchStatuses(); }, [bound.data, refetchStatuses]);
  // Tonight for each rig: its clear dark hours and what it plans to shoot.
  const tonight = useQuery({
    queryKey: ['nightPlansTonight'],
    queryFn: () => apiClient.getNightPlans({ nights: 1, schedule_nights: 1 }),
    retry: retryWhenBusy,
    retryDelay: 1200,
    staleTime: 5 * 60_000,
    refetchInterval: current => refetchKept(current.state.data, 10 * 60_000),
    refetchOnWindowFocus: false,
  });
  const workspaceHref = (projectId: string) => planHref(projectId, params);
  const rows = statuses.data ?? [];
  const age = (at: number | null | undefined) => at ? formatAge(now - at) : 'never';
  // Each forecast provider's credit, once for the page rather than on every rig.
  const credits = [...new Map<ForecastProvider, string>([
    ...rows.flatMap(view => view.weather?.forecast ? [[view.weather.forecast.provider, view.weather.forecast.attribution] as const] : []),
    ...(tonight.data?.rigs ?? []).flatMap(rig => rig.forecast ? [[rig.forecast.provider, rig.forecast.attribution] as const] : []),
  ]).entries()];
  return <section className="director-records director-dashboard" aria-label="Live rigs">
    <div className="director-toolbar">
      <div className="early-beta-heading"><h2>Live</h2><EarlyBeta /></div>
      <button type="button" title="Refresh live status" aria-label="Refresh live status" disabled={statuses.isFetching} onClick={() => void statuses.refetch()}><RefreshCw size={16} /></button>
    </div>
    {statuses.isPending && <p role="status">Loading live status...</p>}
    {statuses.isError && <p className="director-error" role="alert">{message(statuses.error)}</p>}
    {statuses.isSuccess && rows.length === 0 && <p className="director-muted">No rigs yet. Add a database under Settings → Databases &amp; rigs.</p>}
    {rows.length > 0 && <div className="director-table-scroll"><table className="director-dashboard-table" data-testid="director-dashboard">
      <thead><tr><th>Rig</th><th>Link</th><th>Now</th><th>Weather</th><th>Program pull</th><th>Check-in</th><th>Status report</th><th>Sync plugin</th><th>Assigned</th><th>Tonight</th></tr></thead>
      <tbody>{rows.map(view => {
        const errors = errorsOf(view);
        const sequence = describeSequence(view.sync, now);
        const recent = describeRecent(view.sync, now);
        // Without a Director report, the Sync plugin's report is the rig's.
        const directorNow = view.status || !sequence ? describeNow(view, now) : null;
        const action = view.sync?.latest_action;
        // Problems the latest action does not already show.
        const problems = (view.sync?.problems ?? []).filter(problem => !action || problem.at_ms !== action.at_ms || problem.action !== action.action);
        return <tr key={view.rig.id} className={`is-${view.connectivity.state}`}>
          <td><span className="director-cell-label">Rig</span><strong>{view.catalog_name ?? view.rig.name}</strong>{view.catalog_name && view.catalog_name !== view.rig.name && <span className="director-muted"> {view.rig.name}</span>}</td>
          <td><span className="director-cell-label">Link</span><span className={`director-connectivity is-${view.connectivity.state}`}>{STATE_LABEL[view.connectivity.state]}</span>{view.connectivity.age_ms !== null && <span className="director-muted"> {formatAge(view.connectivity.age_ms)}</span>}</td>
          <td><span className="director-cell-label">Now</span>{directorNow}{errors.map(error => <span key={error} className="director-error"> {error}</span>)}
            {sequence && <div data-testid="sync-sequence" title={view.sync?.sequence?.operation}>{directorNow && <span className="director-muted">Sync plugin: </span>}{sequence}{recent && <small className="director-muted"> · {recent}</small>}</div>}
            {!!view.recent_operations?.length && <details><summary>Completed operations ({view.recent_operations.length})</summary>
              <ol>{view.recent_operations.map(receipt => {
                const { command, completion } = receipt.event.event.observation;
                return <li key={`${receipt.event.ledger_id}:${receipt.event.sequence}`}>
                  {command.operation.operation.replaceAll('_', ' ')}: {completion.outcome.outcome}, {Math.round(completion.elapsed_ms / 1000)} s
                  {completion.outcome.reason && ` (${completion.outcome.reason})`}
                  <span className="director-muted"> · completed {age(completion.ended_at_ms)} · received {age(receipt.received_at_ms)}</span>
                </li>;
              })}</ol>
            </details>}
          </td>
          <td><span className="director-cell-label">Weather</span><RigWeatherSummary weather={view.weather} now={now} credit={false} /></td>
          <td><span className="director-cell-label">Program pull</span>{age(view.contacts.program_pull?.at_ms)}{view.contacts.program_pull?.detail && <span className="director-muted"> rev {view.contacts.program_pull.detail.slice(0, 8)}</span>}</td>
          <td><span className="director-cell-label">Check-in</span>{age(view.contacts.check_in?.at_ms)}{view.pending_receipts > 0 && <span className="director-muted"> {view.pending_receipts} saved, ungraded</span>}</td>
          <td><span className="director-cell-label">Status report</span>{age(view.contacts.status?.at_ms)}</td>
          <td data-testid="sync-plugin"><span className="director-cell-label">Sync plugin</span>{!view.sync ? <span className="director-muted">not used</span> : <>
            {action ? <span className={action.outcome === 'ok' ? undefined : 'director-error'}>{describeSyncAction(action)} <span className="director-muted">{age(action.at_ms)}</span></span> : <span className="director-muted">no action yet</span>}
            {problems.map(problem => <span key={`${problem.action}:${problem.operation ?? ''}`} className="director-error"><br />{describeSyncAction(problem)} <span className="director-muted">{age(problem.at_ms)}</span></span>)}
            <br /><span className="director-muted">contact {age(view.sync.last_contact_ms)}{view.sync.plugin_version && `, plugin ${view.sync.plugin_version}`}</span>
          </>}</td>
          <td><span className="director-cell-label">Assigned</span>{view.assignments.length === 0 ? <span className="director-muted">nothing activated</span>
            : view.assignments.map((assignment, index) => <span key={assignment.project.id}>{index > 0 && ', '}<Link to={workspaceHref(assignment.project.id)}>{assignment.project.name}</Link></span>)}</td>
          <td><span className="director-cell-label">Tonight</span><TonightCell rig={tonight.data?.rigs.find(entry => entry.rig.id === view.rig.id)} pending={tonight.isPending} failed={tonight.isError ? message(tonight.error) : null} /></td>
        </tr>;
      })}</tbody>
    </table></div>}
    {credits.length > 0 && <p className="director-muted" data-testid="forecast-credits">Forecasts: {credits.map(([provider, attribution], index) => <span key={provider}>{index > 0 && ' · '}<a href={PROVIDER_URL[provider]} target="_blank" rel="noreferrer">{attribution}</a></span>)}</p>}
  </section>;
}

/** A rig's night under way: its clear dark hours and the plans its
 *  runtime would shoot, linked to the night on the Nights page. */
function TonightCell({ rig, pending, failed }: { rig: RigNights | undefined; pending: boolean; failed: string | null }) {
  const { search } = useLocation();
  if (failed) return <span className="director-muted" title={failed}>Not planned</span>;
  if (!rig) return <span className="director-muted">{pending ? 'Planning...' : 'No site'}</span>;
  const night = rig.nights[0];
  if (!night) return null;
  const plans = plansScheduled(rig, night.date);
  const href = nightHref(rig.rig.id, night.date, search);
  const clear = night.availability.state === 'unavailable' ? 'not available' : `${formatHours(night.available_dark_hours)} clear`;
  const forecast = night.availability.marks.some(mark => mark.source.kind === 'forecast') ? rig.forecast : null;
  // The provider's credit is at the foot of the page.
  return <Link to={href} title={forecast ? "Clear hours from the site's cloud forecast." : undefined}>
    {clear}{plans.length > 0 ? `: ${plans.slice(0, 2).map(plan => plan.name).join(', ')}${plans.length > 2 ? ` +${plans.length - 2}` : ''}` : night.available_dark_hours > 0 ? ', nothing planned' : ''}
  </Link>;
}
