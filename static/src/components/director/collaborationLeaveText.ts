import type { DirectorCoordinatorOutcome, DirectorPlanCollaboration } from '../../api/directorTypes';
import { listNames } from './listNames';

const count = (value: number, word: string) => `${value.toLocaleString()} ${word}${value === 1 ? '' : 's'}`;

/** "the collaboration at collab.example (M31 halo, project 000000000002)". */
export function collaborationName(collaboration: DirectorPlanCollaboration): string {
  const project = collaboration.remote_project_name
    ? `${collaboration.remote_project_name}, project ${collaboration.remote_project_id}`
    : `project ${collaboration.remote_project_id}`;
  return `the collaboration at ${collaboration.host} (${project})`;
}

/** What leaving removes, one sentence per item, in the order PSF Guard does it. */
export function leaveChanges(collaboration: DirectorPlanCollaboration): string[] {
  const changes: string[] = [];
  if (collaboration.open_assignments > 0) {
    changes.push(`Ends ${count(collaboration.open_assignments, 'open assignment')} in the rig databases, so new frames no longer count toward the collaboration.`);
  }
  const reports = [
    collaboration.unsent_reports > 0 ? `${count(collaboration.unsent_reports, 'report')} not yet sent` : '',
    collaboration.sent_reports > 0 ? `the record of ${count(collaboration.sent_reports, 'report')} already sent` : '',
  ].filter(Boolean);
  changes.push(`Removes ${count(collaboration.imports, 'imported nightly assignment')}${reports.length ? ` and ${listNames(reports)}` : ''}.`
    + (collaboration.sent_reports > 0 ? ' The coordinator keeps the reports it received.' : ''));
  if (collaboration.transfers > 0) {
    changes.push(`Cancels ${count(collaboration.transfers, 'file transfer')} that ${collaboration.transfers === 1 ? 'has' : 'have'} not finished.`);
  }
  if (collaboration.nightly_intake.length > 0) {
    changes.push(`Stops nightly intake on ${listNames(collaboration.nightly_intake)} from importing the project.`);
  }
  return changes;
}

/** What PSF Guard will tell the coordinator, before leaving. */
export function coordinatorPlan(collaboration: DirectorPlanCollaboration): string {
  return collaboration.coordinator_reachable
    ? `PSF Guard declines the plan's tasks at ${collaboration.host}, so the coordinator stops assigning work for it to this server's rigs.`
    : `PSF Guard cannot tell ${collaboration.host}: no rig here is signed in to it. Leave or decline the project on ${collaboration.host} too.`;
}

/** What telling the coordinator did, after leaving. */
export function coordinatorResult(told: DirectorCoordinatorOutcome): string {
  if (told.problems.length === 0) {
    return `${told.host} declined the plan's ${told.declined.length === 1 ? 'task' : 'tasks'}.`;
  }
  return `${told.host} was not told: ${told.problems.join(' ')} Leave or decline the project on ${told.host} too.`;
}
