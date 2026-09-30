import { useEffect, useRef, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import type { DirectorPlanRow } from '../../api/directorTypes';
import { useCurrentPlan } from './useCurrentPlan';
import { withoutPlanningParams } from '../../hooks/useUrlState';
import { isArchivedPlan, sumFrames } from '../director/planCardModel';
import { matchesSearch } from '../director/planFilters';
import { DbPill, FamilyPill, ProgressPill } from '../projectPills';
import './header.css';

/** The Plan group's scope: which plan the Workspace opens. Lists every plan
 *  with its rigs and progress; a rig chip hops to that rig's project for
 *  review, the way the project picker's family row hops the other way. */
export default function PlanPicker() {
  const plan = useCurrentPlan();
  const navigate = useNavigate();
  const location = useLocation();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => { if (!rootRef.current?.contains(event.target as Node)) setOpen(false); };
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === 'Escape') { setOpen(false); triggerRef.current?.focus(); } };
    document.addEventListener('pointerdown', closeOutside);
    document.addEventListener('keydown', closeOnEscape);
    searchRef.current?.focus();
    return () => { document.removeEventListener('pointerdown', closeOutside); document.removeEventListener('keydown', closeOnEscape); };
  }, [open]);
  const choose = (row: DirectorPlanRow) => { setOpen(false); setSearch(''); navigate(plan.hrefFor(row.project.id)); };
  const review = (slug: string, projectId: number) => {
    setOpen(false); setSearch('');
    const next = withoutPlanningParams(location.search);
    next.set('db', slug); next.set('project', String(projectId)); next.delete('target');
    navigate(`/grid?${next}`);
  };
  // Every plan is in the Library, with its rigs and the plans no database shoots yet.
  const allPlans = () => {
    setOpen(false); setSearch('');
    const query = withoutPlanningParams(location.search).toString();
    navigate(query ? `/?${query}` : '/');
  };
  const shown = plan.rows.filter(row => matchesSearch(row, search));
  const live = shown.filter(row => !isArchivedPlan(row));
  const archived = shown.filter(isArchivedPlan);
  const current = plan.current;
  const scope = plan.loading ? 'Loading plans…' : current ? current.project.name : 'Choose a plan';
  const detail = current ? (current.links.length > 1 ? `${current.links.length} rigs` : current.links[0]?.catalog_name ?? 'no database') : null;
  const option = (row: DirectorPlanRow) => {
    const total = sumFrames(row.links.flatMap(link => link.targets));
    const selected = row.project.id === current?.project.id;
    return <div key={row.project.id} className={`plan-option${selected ? ' is-selected' : ''}`} aria-current={selected ? 'true' : undefined}>
      <button type="button" className="plan-option-main" onClick={() => choose(row)}>
        <span className="plan-option-name">{row.project.name}</span>
        {row.links.length > 1 && <FamilyPill rigs={row.links.length} />}
        {row.links.length === 0 && <span className="library-pill">No database</span>}
        {row.links.length > 0 && <ProgressPill accepted={total.accepted} desired={total.desired} totalImages={total.acquired} />}
      </button>
      {row.links.length > 0 && <span className="plan-option-rigs">
        {row.links.map(link => link.source_row_id === null
          ? <DbPill key={link.catalog_slug} name={link.catalog_name} />
          : <button key={link.catalog_slug} type="button" className="library-pill library-pill-db plan-option-rig" title={`Review ${row.project.name} on ${link.catalog_name}`}
              aria-label={`Review ${row.project.name} on ${link.catalog_name}`} onClick={() => review(link.catalog_slug, link.source_row_id!)}>{link.catalog_name}</button>)}
      </span>}
    </div>;
  };
  return <div ref={rootRef} className="plan-picker selector-picker">
    <button ref={triggerRef} type="button" className="compact-select selector-trigger" aria-label={`Plan: ${scope}`} aria-haspopup="dialog" aria-expanded={open} disabled={plan.loading} onClick={() => setOpen(value => !value)}>
      <span className="selector-trigger-scope"><span>{scope}</span>{detail && <small>{detail}</small>}</span>
      <span aria-hidden="true">▾</span>
    </button>
    {open && <div className="selector-popover plan-picker-popover" role="dialog" aria-label="Choose a plan">
      <input ref={searchRef} type="search" className="selector-search" value={search} onChange={event => setSearch(event.target.value)} placeholder="Type to find a plan" aria-label="Search plans" />
      <div className="selector-options" aria-label="Plans">
        <button type="button" className="selector-option" onClick={allPlans}>
          <span>All plans</span>
          <small>In the Library</small>
        </button>
        {live.map(option)}
        {archived.length > 0 && <details className="selector-archive" open={!!search}><summary>Closed plans <span>{archived.length}</span></summary>{archived.map(option)}</details>}
        {plan.error && <p className="selector-load-error" role="alert">Plans could not be loaded.</p>}
        {!plan.error && shown.length === 0 && <p className="selector-empty">{plan.rows.length === 0 ? 'No plans yet.' : 'No plan matches.'}</p>}
      </div>
    </div>}
  </div>;
}
