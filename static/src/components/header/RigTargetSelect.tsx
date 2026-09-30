import { useMemo } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useMergedProjects, useMergedTargets } from '../../hooks/useDatabases';
import { isMergedPath, useDbProjectTarget, withoutPlanningParams } from '../../hooks/useUrlState';
import { useCurrentPlan } from './useCurrentPlan';
import { fittedWidth, sharedStart, shortLabel } from './targetLabels';
import './header.css';

interface Member { db_id: string; db_name: string; id: number }

const valueOf = (db: string, project: number, target: number | null) => `${db}:${project}:${target ?? ''}`;

/** The second half of the scope: which rig's project Images and Sequence
 *  show, and which of its targets. A plan shot by several rigs lists each
 *  rig with its targets; a lone rig lists its targets alone, and with one
 *  target there is nothing to choose, so it only names the database. */
export default function RigTargetSelect() {
  const location = useLocation();
  const { pathname } = location;
  const navigate = useNavigate();
  const { dbId, projectId, targetId, setDbProjectTarget } = useDbProjectTarget();
  const { current: planInScope } = useCurrentPlan();
  const { data: projects } = useMergedProjects();
  const { data: targets } = useMergedTargets();

  const members: Member[] = useMemo(() => {
    // In a workspace the plan names its rigs; elsewhere the project in scope
    // and every project sharing its Target Scheduler GUID are the rigs.
    if (pathname === '/plan' && planInScope) {
      return planInScope.links
        .filter(link => link.source_row_id !== null)
        .map(link => ({ db_id: link.catalog_slug, db_name: link.catalog_name, id: link.source_row_id! }));
    }
    // The same family rule as the project picker: one GUID (trimmed, any
    // case) held by projects in at least two databases.
    const current = projects.find(project => project.db_id === dbId && project.id === projectId);
    if (!current) return [];
    const key = (guid: string | null | undefined) => guid?.trim().toLowerCase() || null;
    const guid = key(current.guid);
    if (!guid) return [current];
    const kin = projects.filter(project => key(project.guid) === guid);
    return new Set(kin.map(project => project.db_id)).size >= 2 ? kin : [current];
  }, [pathname, planInScope, projects, dbId, projectId]);

  const targetsOf = (member: Member) =>
    targets.filter(target => target.db_id === member.db_id && target.project_id === member.id)
      .sort((a, b) => a.name.localeCompare(b.name));

  // Nothing to choose reads as a label, not a control: a caption over the
  // value, in the muted inset the scope uses for facts.
  if (members.length === 0) {
    return <span className="rig-target-fixed is-empty" aria-label="Rig: none in scope"><small>Rig</small><span>None in scope</span></span>;
  }
  const single = members.length === 1;
  const lone = single ? targetsOf(members[0]) : [];
  if (single && lone.length <= 1) {
    const what = lone[0] ? `${members[0].db_name} · ${lone[0].name}` : members[0].db_name;
    return <span className="rig-target-fixed" aria-label={`Rig and target: ${what}`}
      title="This project has one rig and one target, so there is nothing to choose">
      <small>Rig · target</small><span>{what}</span>
    </span>;
  }
  const inScope = members.some(member => member.db_id === dbId && member.id === projectId);
  // A target the URL still names from another project reads as all targets.
  const knownTarget = inScope && targetId !== null && targets.some(target => target.db_id === dbId && target.project_id === projectId && target.id === targetId) ? targetId : null;
  const value = inScope && dbId !== null && projectId !== null ? valueOf(dbId, projectId, knownTarget) : '';
  // On the Library or the Sky the scope is only parked; choosing a rig there
  // opens Images for it, as the project picker does. Elsewhere it moves the
  // scope in place.
  const choose = (db: string, project: number, target: number | null) => {
    if (isMergedPath(pathname) && pathname !== '/plan') {
      const next = withoutPlanningParams(location.search);
      next.set('db', db); next.set('project', String(project));
      if (target === null) next.delete('target'); else next.set('target', String(target));
      navigate(`/grid?${next}`);
      return;
    }
    setDbProjectTarget(db, project, target);
  };
  // Each option's full and shown text; mosaic panels drop the start they share.
  const labels = new Map<string, { full: string; shown: string }>();
  const groupLabel = new Map<string, string>();
  for (const member of members) {
    const own = targetsOf(member);
    const prefix = sharedStart(own.map(target => target.name));
    groupLabel.set(`${member.db_id}:${member.id}`, prefix ? `${member.db_name} · ${prefix}` : member.db_name);
    const all = single ? 'All targets' : `${member.db_name} · all targets`;
    labels.set(valueOf(member.db_id, member.id, null), { full: prefix ? `${all} (${prefix})` : all, shown: all });
    for (const target of own) {
      labels.set(valueOf(member.db_id, member.id, target.id), { full: target.name, shown: shortLabel(target.name, prefix) });
    }
  }
  const options = (member: Member) => [
    valueOf(member.db_id, member.id, null),
    ...targetsOf(member).map(target => valueOf(member.db_id, member.id, target.id)),
  ].map(key => {
    const label = labels.get(key)!;
    return <option key={key} value={key} title={label.full}>{label.shown}</option>;
  });
  const selected = labels.get(value);
  // As wide as the chosen option needs, within reason, so a closed select
  // shows its name whole.
  const shownText = selected?.shown ?? 'Choose a rig';
  return <select className="compact-select rig-target-select" aria-label="Rig and target" value={value}
    title={selected?.full} style={{ width: fittedWidth(shownText) }}
    onChange={event => {
      const [db, project, target] = event.target.value.split(':');
      if (db && project) choose(db, Number(project), target ? Number(target) : null);
    }}>
    {!inScope && <option value="" disabled>Choose a rig</option>}
    {single ? options(members[0]) : members.map(member => <optgroup key={`${member.db_id}:${member.id}`} label={groupLabel.get(`${member.db_id}:${member.id}`)}>{options(member)}</optgroup>)}
  </select>;
}
