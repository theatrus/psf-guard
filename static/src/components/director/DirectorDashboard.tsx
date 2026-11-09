import { Fragment, useEffect, useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
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
  const [historyOpen, setHistoryOpen] = useState<ReadonlySet<string>>(() => new Set());
  const toggleHistory = (rig: string) => setHistoryOpen(current => {
    const next = new Set(current);
    if (!next.delete(rig)) next.add(rig);
    return next;
  });
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
      <thead><tr><th>Rig</th><th>Now</th><th>Weather</th><th>Plugins</th><th>Projects</th><th>Tonight</th></tr></thead>
      <tbody>{rows.map(view => {
        const errors = errorsOf(view);
        const sequence = describeSequence(view.sync, now);
        const recent = describeRecent(view.sync, now);
        // Without a Director report, the Sync plugin's report is the rig's.
        const directorNow = view.status || !sequence ? describeNow(view, now) : null;
        const problems = view.sync?.problems ?? [];
        const { program_pull: pull, check_in: checkIn, status: reported } = view.contacts;
        const plugins = [
          (pull || checkIn || reported) && { name: 'Director', title: `Program pull ${age(pull?.at_ms)}${pull?.detail ? ` (rev ${pull.detail.slice(0, 8)})` : ''}, check-in ${age(checkIn?.at_ms)}, status report ${age(reported?.at_ms)}` },
          view.sync?.last_contact_ms && { name: 'Sync', title: `Contact ${age(view.sync.last_contact_ms)}${view.sync.plugin_version ? `, plugin ${view.sync.plugin_version}` : ''}` },
        ].filter((plugin): plugin is { name: string; title: string } => !!plugin);
        const slug = view.catalog_slug;
        const historyShown = !!slug && historyOpen.has(view.rig.id);
        return <Fragment key={view.rig.id}><tr className={`is-${view.connectivity.state}`}>
          <td><span className="director-cell-label">Rig</span><strong>{view.catalog_name ?? view.rig.name}</strong>{view.catalog_name && view.catalog_name !== view.rig.name && <span className="director-muted"> {view.rig.name}</span>}
            <span className="live-link"><span className={`director-connectivity is-${view.connectivity.state}`}>{STATE_LABEL[view.connectivity.state]}</span>{view.connectivity.age_ms !== null && <span className="director-muted"> {formatAge(view.connectivity.age_ms)}</span>}</span>
          </td>
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
          <td><span className="director-cell-label">Weather</span><RigWeatherSummary weather={view.weather} now={now} compact /></td>
          <td data-testid="rig-plugins"><span className="director-cell-label">Plugins</span>
            {plugins.length === 0 ? <span className="director-muted">No plugin has called</span>
              : plugins.map((plugin, index) => <span key={plugin.name} title={plugin.title}>{index > 0 && ' · '}{plugin.name}</span>)}
            {view.pending_receipts > 0 && <span className="director-muted"> · {view.pending_receipts} saved, ungraded</span>}
            {problems.map(problem => <div key={`${problem.action}:${problem.operation ?? ''}`} className="director-error">{describeSyncAction(problem)} <span className="director-muted">{age(problem.at_ms)}</span></div>)}
            {slug && (plugins.length > 0 || view.sync) && <div><button type="button" className="director-link-button" aria-expanded={historyShown} aria-controls={`history-${view.rig.id}`} onClick={() => toggleHistory(view.rig.id)}>History</button></div>}
          </td>
          <td><span className="director-cell-label">Projects</span>{slug
            ? <Link to={`/?${new URLSearchParams({ db: slug, dbfilter: slug, show: 'active' })}`}>{view.active_projects == null ? 'Active projects' : `${view.active_projects} active`}</Link>
            : <span className="director-muted">no database</span>}</td>
          <td><span className="director-cell-label">Tonight</span><TonightCell rig={tonight.data?.rigs.find(entry => entry.rig.id === view.rig.id)} pending={tonight.isPending} failed={tonight.isError ? message(tonight.error) : null} /></td>
        </tr>
        {historyShown && slug && <tr className="live-history-row"><td colSpan={6} id={`history-${view.rig.id}`}><PluginHistory slug={slug} now={now} /></td></tr>}
        </Fragment>;
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

/** The plugins' recent actions on one rig's database, newest first. */
function PluginHistory({ slug, now }: { slug: string; now: number }) {
  const history = useQuery({ queryKey: ['pluginHistory', slug], queryFn: () => apiClient.getPluginHistory(slug, 30), refetchInterval: 30_000, refetchOnWindowFocus: false });
  if (history.isPending) return <p role="status">Loading history...</p>;
  if (history.isError) return <p className="director-error" role="alert">{message(history.error)}</p>;
  if (history.data.length === 0) return <p className="director-muted">No plugin actions recorded yet.</p>;
  return <ol className="live-history" aria-label="Plugin history">{history.data.map((entry, index) => <li key={`${entry.at_ms}:${index}`} className={entry.outcome === 'ok' ? undefined : 'director-error'}>
    <span className="director-muted">{new Date(entry.at_ms).toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })} · {formatAge(now - entry.at_ms)}</span> {describeSyncAction(entry)}
  </li>)}</ol>;
}
