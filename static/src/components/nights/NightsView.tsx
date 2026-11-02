import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { RefreshCw } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { RigNight, RigNights } from '../../api/nightPlanTypes';
import { useAccess } from '../../auth/access';
import { useUrlParams } from '../../hooks/useUrlState';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import { retryWhenBusy } from '../director/retry';
import { formatHours } from '../director/visibilityFormat';
import EarlyBeta from '../director/EarlyBeta';
import NightDetail from './NightDetail';
import {
  NIGHT_COUNTS,
  NIGHTS_PARAMS,
  RUNTIME_LABEL,
  STATE_LABEL,
  availableShares,
  datesBetween,
  eveningOf,
  formatNightDate,
  isDate,
  isTonight,
  loadWords,
  localDate,
  markGlyph,
  planColours,
  plansOnNight,
  plansScheduled,
  scheduleDone,
} from './nightsModel';
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
export default function NightsView() {
  const { getParam, updateParams } = useUrlParams();
  const { canWrite } = useAccess();
  const now = useDirectorClock();
  const today = localDate(new Date(now));
  const from = isDate(getParam(NIGHTS_PARAMS.from)) ? getParam(NIGHTS_PARAMS.from)! : today;
  const countParam = Number(getParam(NIGHTS_PARAMS.count));
  const count = (NIGHT_COUNTS as readonly number[]).includes(countParam) ? countParam : NIGHT_COUNTS[0];
  const hidden = useMemo(() => new Set((getParam(NIGHTS_PARAMS.hide) ?? '').split(',').filter(Boolean)), [getParam]);
  // A link may open one rig's night.
  const [selection, setSelection] = useState<Selection | null>(() => {
    const rig = getParam(NIGHTS_PARAMS.rig);
    const night = getParam(NIGHTS_PARAMS.night);
    return rig && isDate(night) ? { rigId: rig, anchor: night, dates: [night] } : null;
  });
  const query = useQuery({
    queryKey: ['nightPlans', from, count],
    queryFn: () => apiClient.getNightPlans({ start_ms: eveningOf(from), nights: count, schedule_nights: count }),
    retry: retryWhenBusy,
    retryDelay: 1200,
    refetchInterval: 5 * 60_000,
    refetchOnWindowFocus: false,
  });
  const rigs = useMemo(() => query.data?.rigs ?? [], [query.data]);
  const shown = rigs.filter(rig => !hidden.has(rig.rig.id));
  const colours = useMemo(() => planColours(rigs), [rigs]);
  const dates = rigs[0]?.nights.map(night => night.date) ?? [];
  const selectedRig = selection ? rigs.find(rig => rig.rig.id === selection.rigId) ?? null : null;
  const selectedNights = selectedRig && selection
    ? selectedRig.nights.filter(night => selection.dates.includes(night.date))
    : [];

  const choose = (rig: RigNights, night: RigNight, extend: boolean) => {
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
          <label>From <input type="date" value={from} onChange={event => { setSelection(null); updateParams({ [NIGHTS_PARAMS.from]: isDate(event.target.value) && event.target.value !== today ? event.target.value : null }); }} /></label>
          <label>Nights <select value={count} onChange={event => { setSelection(null); updateParams({ [NIGHTS_PARAMS.count]: Number(event.target.value) === NIGHT_COUNTS[0] ? null : event.target.value }); }}>
            {NIGHT_COUNTS.map(option => <option key={option} value={option}>{option}</option>)}
          </select></label>
          {from !== today && <button type="button" onClick={() => { setSelection(null); updateParams({ [NIGHTS_PARAMS.from]: null }); }}>Tonight</button>}
          <button type="button" title="Compute again" aria-label="Compute again" disabled={query.isFetching} onClick={() => void query.refetch()}><RefreshCw size={16} /></button>
        </div>
      </div>
      <p className="director-muted">
        Each row is a rig and each column a night, from dusk to dawn. A bar shows the dark hours the night is clear for,
        and the line under it how much of what the rig owes fits. Choose a night to see its plans{canWrite ? ' and to mark it' : ''};
        Shift-click another night of the same rig to choose a run of nights.
        These are planning estimates: the rig still decides on the night.
      </p>
      {rigs.length > 1 && <div className="nights-rig-chips" role="group" aria-label="Rigs shown">
        {rigs.map(rig => <button key={rig.rig.id} type="button" className="nights-chip" aria-pressed={!hidden.has(rig.rig.id)} onClick={() => toggleRig(rig.rig.id)}>{rig.catalog_name}</button>)}
      </div>}
      {query.isPending && <p role="status">Computing the nights...</p>}
      {query.isError && <div role="alert"><p>{message(query.error)}</p><button type="button" onClick={() => void query.refetch()}>Retry</button></div>}
      {query.data?.warnings.map(warning => <p key={warning} className="director-muted" role="note">{warning}</p>)}
      {query.isSuccess && rigs.length === 0 && <p className="director-muted">No rig to plan yet. Give a rig a site in its setup and its nights show here.</p>}
      {shown.length > 0 && <div className="nights-grid-scroll">
        <table className="nights-grid" data-testid="nights-grid">
          <thead><tr>
            <th scope="col" className="nights-rig-head">Rig</th>
            {dates.map(date => <th key={date} scope="col" className={rigs[0]?.nights.find(n => n.date === date && isTonight(n, now)) ? 'is-tonight' : undefined}>
              <span className="nights-date">{formatNightDate(date)}</span>
            </th>)}
          </tr></thead>
          <tbody>{shown.map(rig => <tr key={rig.rig.id}>
            <th scope="row" className="nights-rig">
              <strong>{rig.catalog_name}</strong>
              <span className="director-muted">{RUNTIME_LABEL[rig.runtime]}{rig.planning_site ? ` · ${rig.planning_site.name}` : ''}</span>
              <span className="director-muted">{rig.plans.length === 0 ? 'Nothing owed' : `${rig.plans.length} plan${rig.plans.length === 1 ? '' : 's'} owed`}</span>
            </th>
            {rig.nights.map(night => {
              const chosen = selection?.rigId === rig.rig.id && selection.dates.includes(night.date);
              // What the rig's runtime plans to shoot, else what could fit.
              const scheduled = rig.schedule ? plansScheduled(rig, night.date) : null;
              const working = scheduled ?? plansOnNight(rig, night).map(plan => ({ name: plan.name, hours: plan.hours }));
              const plannedHours = scheduled?.reduce((sum, plan) => sum + plan.hours, 0) ?? null;
              const glyph = markGlyph(night);
              return <td key={night.date}>
                <button type="button" className={`nights-cell is-${night.availability.state}`} aria-pressed={chosen}
                  aria-label={`${rig.catalog_name}, ${formatNightDate(night.date)}: ${formatHours(night.available_dark_hours)} clear of ${formatHours(night.dark_hours)} dark, ${STATE_LABEL[night.availability.state].toLowerCase()}, Moon ${Math.round(night.moon_illumination * 100)}%`}
                  onClick={event => choose(rig, night, event.shiftKey)}>
                  <span className="nights-cell-top">
                    <span className="nights-cell-hours">{formatHours(night.available_dark_hours)}</span>
                    {glyph && <span className="nights-cell-mark" aria-hidden="true">{glyph}</span>}
                    <span className="nights-cell-moon" title={`Moon ${Math.round(night.moon_illumination * 100)}% lit, up ${formatHours(night.moon_hours_up_in_dark)} of the dark`}>☾ {Math.round(night.moon_illumination * 100)}%</span>
                  </span>
                  <span className="nights-cell-bar" aria-hidden="true">
                    {availableShares(night).map(part => <span key={part.from} className="nights-cell-clear" style={{ left: `${part.from * 100}%`, width: `${(part.to - part.from) * 100}%` }} />)}
                  </span>
                  <span className="nights-cell-plans" aria-hidden="true">
                    {working.slice(0, 6).map(plan => <span key={plan.name} className="nights-plan-dot" data-colour={colours.get(plan.name) ?? 0} title={`${plan.name}: ${formatHours(plan.hours)}`} />)}
                    {working.length > 6 && <span className="nights-plan-more">+{working.length - 6}</span>}
                  </span>
                  <span className={`nights-cell-load${plannedHours === null && night.load.overbooked_hours >= 0.05 ? ' is-over' : ''}`}>{plannedHours === null ? loadWords(night) : plannedHours > 0 ? `${formatHours(plannedHours)} planned` : scheduleDone(rig, night.date) ? 'all done' : night.available_dark_hours > 0 ? 'nothing planned' : ''}</span>
                </button>
              </td>;
            })}
          </tr>)}</tbody>
        </table>
      </div>}
      {selectedRig && selectedNights.length > 0 && <NightDetail
        key={`${selectedRig.rig.id}:${selectedNights.map(n => n.date).join(',')}`}
        rig={selectedRig}
        nights={selectedNights}
        colours={colours}
        canMark={canWrite}
        range={{ from, count }}
        onClose={() => setSelection(null)}
      />}
    </section>
  );
}
