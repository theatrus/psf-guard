/** What the Library can narrow to. A project shot by several rigs is one
 *  family: it matches a state when any rig has it, since the plan is live
 *  wherever one rig still shoots it; Closed and Done ask for every rig. */
export type LibraryShow = 'all' | 'active' | 'open' | 'done' | 'draft' | 'inactive' | 'closed' | 'unlinked';

export const SHOW_OPTIONS: ReadonlyArray<{ value: LibraryShow; label: string; planning?: true }> = [
  { value: 'all', label: 'All projects' },
  { value: 'active', label: 'Active' },
  { value: 'open', label: 'Still to shoot' },
  { value: 'done', label: 'Done' },
  { value: 'draft', label: 'Draft' },
  { value: 'inactive', label: 'Inactive' },
  { value: 'closed', label: 'Closed' },
  // Only Planning has projects without a database.
  { value: 'unlinked', label: 'No database', planning: true },
];

export function parseShow(value: string | null, planning: boolean): LibraryShow {
  const option = SHOW_OPTIONS.find(entry => entry.value === value);
  return option && (planning || !option.planning) ? option.value : 'all';
}

interface Member { state: number; accepted_images: number; total_desired: number }

/** A plan the Library has no row for: nothing captured yet, so its rigs'
 *  project states are all there is to go on, and it cannot be done. Closed
 *  on every rig, it stays hidden until Closed is asked for, the way the
 *  Library folds closed projects into its archive. */
export function waitingPlanMatchesShow(links: { source_state?: number | null }[], show: LibraryShow): boolean {
  if (links.length === 0) return show === 'all' || show === 'unlinked';
  const states = links.map(link => link.source_state);
  const closed = states.every(state => state === 3);
  switch (show) {
    case 'all': return !closed;
    case 'active': return states.includes(1);
    case 'inactive': return states.includes(2);
    case 'draft': return states.includes(0);
    case 'closed': return closed;
    case 'open': return !closed;
    case 'done': return false;
    case 'unlinked': return false;
  }
}

/** The goal is met: accepted frames reach the desired count. No goal is never done. */
export function isDoneProject(project: Member): boolean {
  return project.total_desired > 0 && project.accepted_images >= project.total_desired;
}

export function familyMatchesShow(members: Member[], show: LibraryShow): boolean {
  if (members.length === 0) return show === 'all' || show === 'unlinked';
  const any = (state: number) => members.some(member => member.state === state);
  const closed = members.every(member => member.state === 3);
  const done = members.every(isDoneProject);
  switch (show) {
    case 'all': return true;
    case 'active': return any(1);
    case 'inactive': return any(2);
    case 'draft': return any(0);
    case 'closed': return closed;
    case 'done': return done;
    case 'open': return !done && !closed;
    case 'unlinked': return false;
  }
}
