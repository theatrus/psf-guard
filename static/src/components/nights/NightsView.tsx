import { useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Cloud, Droplet, Pin, RefreshCw } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { RigNight, RigNights } from '../../api/nightPlanTypes';
import { useAccess } from '../../auth/access';
import { useUrlParams } from '../../hooks/useUrlState';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import { retryWhenBusy } from '../director/retry';
import { formatHours } from '../director/visibilityFormat';
import EarlyBeta from '../director/EarlyBeta';
import { PROVIDER_URL } from '../director/forecastLabels';
import NightDetail from './NightDetail';
import { confirmDiscard, usePageDrafts } from '../director/pageDraftsState';
import {
  darkState,
  formatClock,
  refetchKept,
  NIGHT_COUNTS,
  NIGHTS_PARAMS,
  RUNTIME_LABEL,
  STATE_LABEL,
  availableShares,
  datesBetween,
  formatNightDate,
  isDate,
  isTonight,
  loadWords,
  localDate,
  causeWords,
  skyLabels,
  skyRows,
  decidedBy,
  forecastCauses,
  forecastMark,
  markGlyph,
  markOrigin,
  planColours,
  plansOnNight,
  plansScheduled,
  scheduleDone,
  siteGroups,
  usableZone,
} from './nightsModel';
import NightSky, { SkyLabels } from './NightSky';
import './NightsView.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Night plans could not be computed';

/** The rig and nights a person picked: one night, or a run of nights on
 *  one rig for marking them together. */
interface Selection {
  rigId: string;
  anchor: string;
  dates: string[];
}

/**
 * Every rig across the coming nights, on the Rigs page: how long each night
 * is dark and clear, what the Moon does, and what each rig would shoot.
 * Choose a night to see its schedule and plans, and to mark it available,
 * out, or clear for some hours.
 */
/** How many targets the rig has frames left for. */
function describeTargets(rig: RigNights): string {
  const targets = rig.plans.reduce((sum, project) => sum + project.targets.length, 0);
  return targets === 0 ? 'No targets left' : `${targets} target${targets === 1 ? '' : 's'}`;
}

export default function NightsView() {
  const { getParam, updateParams } = useUrlParams();
  const { canWrite } = useAccess();
  const now = useDirectorClock();
  const today = localDate(new Date(now));
  // A chosen first night is the same date at every rig. Without one, each
  // rig starts with the night under way at its own site.
  const chosenFrom = isDate(getParam(NIGHTS_PARAMS.from)) ? getParam(NIGHTS_PARAMS.from)! : null;
  const from = chosenFrom ?? today;
  const countParam = Number(getParam(NIGHTS_PARAMS.count));
  const count = (NIGHT_COUNTS as readonly number[]).includes(countParam) ? countParam : NIGHT_COUNTS[0];
  const hidden = useMemo(() => new Set((getParam(NIGHTS_PARAMS.hide) ?? '').split(',').filter(Boolean)), [getParam]);
  // A link may open one rig's night.
  const [selection, setSelection] = useState<Selection | null>(() => {
    const rig = getParam(NIGHTS_PARAMS.rig);
    const night = getParam(NIGHTS_PARAMS.night);
    return rig && isDate(night) ? { rigId: rig, anchor: night, dates: [night] } : null;
  });
  // Compute again asks the server to compute the plans afresh; any other
  // read gets the plans it keeps current.
  const recompute = useRef(false);
  const query = useQuery({
    queryKey: ['nightPlans', chosenFrom, count],
    // A retry after a failure still asks afresh.
    queryFn: async () => {
      const refresh = recompute.current;
      const planned = await apiClient.getNightPlans({ ...(chosenFrom ? { first_night: chosenFrom } : {}), nights: count, schedule_nights: count, ...(refresh ? { refresh } : {}) });
      if (refresh) recompute.current = false;
      return planned;
    },
    retry: retryWhenBusy,
    retryDelay: 1200,
    refetchInterval: current => refetchKept(current.state.data, 60_000),
    refetchOnWindowFocus: false,
  });
  const rigs = useMemo(() => query.data?.rigs ?? [], [query.data]);
  const shown = rigs.filter(rig => !hidden.has(rig.rig.id));
  // Each forecast provider whose data the grid shows, credited once.
  const credits = [...new Map(shown.filter(rig => rig.forecast?.fetched_at_ms).map(rig => [rig.forecast!.provider, {
    text: rig.forecast!.attribution, url: PROVIDER_URL[rig.forecast!.provider],
  }])).values()];
  const colours = useMemo(() => planColours(rigs), [rigs]);
  const groups = siteGroups(shown);
  // Columns by date across the rigs shown: rigs far apart can be on
  // different nights at the same instant.
  const dates = [...new Set(shown.flatMap(rig => rig.nights.map(night => night.date)))].sort();
  const selectedRig = selection ? rigs.find(rig => rig.rig.id === selection.rigId) ?? null : null;
  const selectedNights = selectedRig && selection
    ? selectedRig.nights.filter(night => selection.dates.includes(night.date))
    : [];

  // The open night's unsaved edits, kept here so choosing another night,
  // closing it or changing the range asks before dropping them.
  const drafts = usePageDrafts();
  const leaving = (then: () => void) => () => { if (confirmDiscard(drafts)) then(); };
  const choose = (rig: RigNights, night: RigNight, extend: boolean) => {
    if (!confirmDiscard(drafts)) return;
    if (extend && selection && selection.rigId === rig.rig.id) {
      setSelection({ ...selection, dates: datesBetween(rig, selection.anchor, night.date) });
    } else if (selection && selection.rigId === rig.rig.id && selection.dates.length === 1 && selection.dates[0] === night.date) {
      setSelection(null);
    } else {
      setSelection({ rigId: rig.rig.id, anchor: night.date, dates: [night.date] });
    }
  };
  const toggleRig = (id: string) => {
    const next = new Set(hidden);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    updateParams({ [NIGHTS_PARAMS.hide]: next.size > 0 ? [...next].join(',') : null });
  };

  return (
    <section className="nights-page" aria-label="Nights">
      <div className="director-toolbar">
        <div className="early-beta-heading"><h2>Nights</h2><EarlyBeta /></div>
        <div className="nights-controls">
          <label>From <input type="date" value={from} onChange={event => { if (!confirmDiscard(drafts)) return; setSelection(null); updateParams({ [NIGHTS_PARAMS.from]: isDate(event.target.value) && event.target.value !== today ? event.target.value : null }); }} /></label>
          <label>Nights <select value={count} onChange={event => { if (!confirmDiscard(drafts)) return; setSelection(null); updateParams({ [NIGHTS_PARAMS.count]: Number(event.target.value) === NIGHT_COUNTS[0] ? null : event.target.value }); }}>
            {NIGHT_COUNTS.map(option => <option key={option} value={option}>{option}</option>)}
          </select></label>
          {chosenFrom && <button type="button" onClick={leaving(() => { setSelection(null); updateParams({ [NIGHTS_PARAMS.from]: null }); })}>Tonight</button>}
          {query.data && <span className="director-muted nights-updated" role="status">{query.data.refreshing || query.isFetching ? 'Updating…' : `Updated ${formatClock(query.data.computed_at_ms)}`}</span>}
          <button type="button" title="Compute again" aria-label="Compute again" disabled={query.isFetching} onClick={() => { recompute.current = true; void query.refetch(); }}><RefreshCw size={16} /></button>
        </div>
      </div>
      {rigs.length > 1 && <div className="nights-rig-chips" role="group" aria-label="Rigs shown">
        {rigs.map(rig => <button key={rig.rig.id} type="button" className="nights-chip" aria-pressed={!hidden.has(rig.rig.id)} onClick={() => toggleRig(rig.rig.id)}>{rig.catalog_name}</button>)}
      </div>}
      {query.isPending && <p role="status">Computing the nights...</p>}
      {query.isError && <div role="alert"><p>{message(query.error)}</p><button type="button" onClick={() => void query.refetch()}>Retry</button></div>}
      {query.data?.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
      {query.isSuccess && rigs.length === 0 && <p className="director-muted">No rigs to show. Give a rig a site in Rig setup.</p>}
      {shown.length > 0 && <div className="nights-grid-scroll">
        <table className="nights-grid" data-testid="nights-grid">
          <thead><tr>
            <th scope="col" className="nights-rig-head">Rig</th>
            {dates.map(date => <th key={date} scope="col" className={shown.some(rig => rig.nights.some(n => n.date === date && isTonight(n, now))) ? 'is-tonight' : undefined}>
              <span className="nights-date">{formatNightDate(date)}</span>
            </th>)}
          </tr></thead>
          {groups.map(group => <tbody key={group.key} aria-label={group.name}>
            <tr className="nights-group-head"><th scope="rowgroup" colSpan={dates.length + 1}>
              {group.name}{group.name !== group.coordinates && <span className="director-muted"> · {group.coordinates}</span>}
            </th></tr>
            {group.rigs.map(rig => <tr key={rig.rig.id}>
            <th scope="row" className="nights-rig">
              <strong>{rig.catalog_name}</strong>
              <span className="director-muted">{RUNTIME_LABEL[rig.runtime]}{rig.planning_site ? ` · ${rig.planning_site.name}` : ''}</span>
              <span className="director-muted">{describeTargets(rig)}</span>
            </th>
            {dates.map(date => {
              const night = rig.nights.find(n => n.date === date);
              if (!night) return <td key={date} />;
              const chosen = selection?.rigId === rig.rig.id && selection.dates.includes(night.date);
              // What the rig's runtime plans to shoot, else what could fit.
              const scheduled = rig.schedule ? plansScheduled(rig, night.date) : null;
              const working = scheduled ?? plansOnNight(rig, night).map(plan => ({ name: plan.name, hours: plan.hours }));
              const plannedHours = scheduled?.reduce((sum, plan) => sum + plan.hours, 0) ?? null;
              const glyph = markGlyph(night);
              const forecast = forecastMark(night);
              const decided = decidedBy(night);
              // Cloud, dew or both: what the forecast took out of the dark hours.
              const causes = forecast && forecast.state !== 'available' ? forecastCauses(rig, night) : { cloud: false, dew: false };
              const why = causeWords(causes);
              const cause = (decided === 'forecast' ? ' by the forecast' : decided === 'both' ? ' by a mark and the forecast' : '') + (decided !== 'none' && decided !== 'manual' && why ? `: ${why}` : '');
              return <td key={date}>
                <button type="button" className={`nights-cell is-${darkState(night)}`} aria-pressed={chosen}
                  aria-label={`${rig.catalog_name}, ${formatNightDate(night.date)}: ${formatHours(night.available_dark_hours)} clear of ${formatHours(night.dark_hours)} dark, ${STATE_LABEL[darkState(night)].toLowerCase()}${cause}, Moon ${Math.round(night.moon_illumination * 100)}%${night.override_count > 0 ? `, ${night.override_count} ${night.override_count === 1 ? 'override' : 'overrides'}` : ''}`}
                  onClick={event => choose(rig, night, event.shiftKey)}>
                  <span className="nights-cell-top">
                    <span className="nights-cell-hours">{formatHours(night.available_dark_hours)}</span>
                    {glyph && <span className="nights-cell-mark" aria-hidden="true">{glyph}</span>}
                    {night.override_count > 0 && <span className="nights-cell-override" aria-hidden="true" title={`${night.override_count} ${night.override_count === 1 ? 'override' : 'overrides'} for this night`}><Pin size={12} /></span>}
                    {forecast && <span className="nights-cell-forecast" aria-hidden="true" title={`${markOrigin(forecast)}: ${STATE_LABEL[forecast.state].toLowerCase()}${why ? `, ${why}` : ''}. ${rig.forecast?.attribution ?? ''}`}>
                      {(causes.cloud || !causes.dew) && <Cloud size={12} />}
                      {causes.dew && <Droplet size={12} className="nights-cell-dew" />}
                    </span>}
                    <span className="nights-cell-moon" title={`Moon ${Math.round(night.moon_illumination * 100)}% lit, up ${formatHours(night.moon_hours_up_in_dark)}`}>☾ {Math.round(night.moon_illumination * 100)}%</span>
                  </span>
                  <span className="nights-cell-bar" aria-hidden="true">
                    {availableShares(night).map(part => <span key={part.from} className="nights-cell-clear" style={{ left: `${part.from * 100}%`, width: `${(part.to - part.from) * 100}%` }} />)}
                  </span>
                  <span className="nights-cell-plans" aria-hidden="true">
                    {working.slice(0, 6).map(plan => <span key={plan.name} className="nights-plan-dot" data-colour={colours.get(plan.name) ?? 0} title={`${plan.name}: ${formatHours(plan.hours)}`} />)}
                    {working.length > 6 && <span className="nights-plan-more">+{working.length - 6}</span>}
                  </span>
                  <span className="nights-cell-load">{plannedHours === null ? loadWords(night) : plannedHours > 0 ? `${formatHours(plannedHours)} planned` : scheduleDone(rig, night.date) ? 'all done' : night.available_dark_hours > 0 ? 'nothing planned' : ''}</span>
                </button>
              </td>;
            })}
          </tr>)}
            {(() => {
              const rows = skyRows(group.rigs);
              return <tr className="nights-sky-row">
                <th scope="row" className="nights-rig" aria-label={skyLabels(rows).join(', ')}><SkyLabels rows={rows} /></th>
                {dates.map(date => <td key={date}><NightSky rigs={group.rigs} rows={rows} date={date} zone={usableZone(group.rigs[0].time_zone)} /></td>)}
              </tr>;
            })()}
          </tbody>)}
        </table>
      </div>}
      {credits.length > 0 && <p className="director-muted nights-credits">Forecasts: {credits.map((credit, index) => <span key={credit.url}>{index > 0 && ', '}<a href={credit.url} target="_blank" rel="noreferrer">{credit.text}</a></span>)}.</p>}
      {selectedRig && selectedNights.length > 0 && <NightDetail
        key={`${selectedRig.rig.id}:${selectedNights.map(n => n.date).join(',')}`}
        rig={selectedRig}
        nights={selectedNights}
        colours={colours}
        canMark={canWrite}
        range={{ first: chosenFrom, count }}
        drafts={drafts}
        onClose={leaving(() => setSelection(null))}
      />}
    </section>
  );
}
