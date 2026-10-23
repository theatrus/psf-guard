import type { DirectorCollaborationLeft, DirectorCoordinatorOutcome, DirectorPlanCollaboration } from '../../api/directorTypes';
import { listNames } from './listNames';

const count = (value: number, word: string) => `${value.toLocaleString()} ${word}${value === 1 ? '' : 's'}`;
const isAre = (names: string[]) => names.length === 1 ? 'is' : 'are';

/** "the collaboration at collab.example (M31 halo, project 000000000002)". */
export function collaborationName(collaboration: DirectorPlanCollaboration): string {
  const project = collaboration.remote_project_name
    ? `${collaboration.remote_project_name}, project ${collaboration.remote_project_id}`
    : `project ${collaboration.remote_project_id}`;
  return `the collaboration at ${collaboration.host} (${project})`;
}

/** What leaving removes, in the order PSF Guard does it. Deleting the plan
 *  deletes its assignments with the rig projects instead of ending them. */
export function leaveChanges(collaboration: DirectorPlanCollaboration, deleting = false): string[] {
  const changes: string[] = [];
  const open = collaboration.open_assignments;
  if (open > 0) {
    changes.push(deleting
      ? `Its ${count(open, 'open assignment')} in the rig databases ${open === 1 ? 'is' : 'are'} deleted with the rig projects.`
      : `Ends ${count(open, 'open assignment')} in the rig databases, so new frames no longer count toward the collaboration.`);
  }
  changes.push(`Removes ${count(collaboration.imports, 'imported nightly assignment')}.`);
  const unsent = collaboration.unsent_reports;
  if (unsent > 0) {
    const them = unsent === 1 ? 'it' : 'them';
    changes.push(`Deletes ${count(unsent, 'report')} not yet sent. ${unsent === 1 ? 'It' : 'They'} will never reach the coordinator. `
      + `To have ${them} counted, deliver ${them} first with Check in in the rig's Collaboration section.`);
  }
  if (collaboration.sent_reports > 0) {
    changes.push(`Removes the record of ${count(collaboration.sent_reports, 'report')} already sent. The coordinator keeps the reports it received.`);
  }
  const frames = collaboration.reported_frames;
  if (frames > 0) {
    changes.push(`${count(frames, 'frame')} in reports ${frames === 1 ? 'loses its' : 'lose their'} protection. `
      + `Removing rejects and deleting the project in the Library can then remove ${frames === 1 ? 'it' : 'them'}.`);
  }
  if (collaboration.transfers > 0) {
    changes.push(`Cancels ${count(collaboration.transfers, 'file transfer')} that ${collaboration.transfers === 1 ? 'has' : 'have'} not finished.`);
  }
  if (collaboration.nightly_intake.length > 0) {
    changes.push(`Stops nightly intake on ${listNames(collaboration.nightly_intake)} from importing the project.`);
  }
  return changes;
}

/** What PSF Guard will tell the coordinator, before leaving: which rigs'
 *  tasks it declines, and which the person must decline there. */
export function coordinatorPlan(collaboration: DirectorPlanCollaboration): string {
  const { host, signed_in: signedIn, not_signed_in: notSignedIn } = collaboration;
  if (notSignedIn.length === 0) {
    return signedIn.length
      ? `PSF Guard declines the tasks of ${listNames(signedIn)} at ${host}, so the coordinator stops assigning this work to ${signedIn.length === 1 ? 'it' : 'them'}.`
      : `${host} has no task for this plan to decline.`;
  }
  const missing = `${listNames(notSignedIn)} ${isAre(notSignedIn)} not signed in to ${host}`;
  if (signedIn.length === 0) {
    return `PSF Guard cannot tell ${host}: ${missing}. Leave or decline the project on ${host} too.`;
  }
  return `PSF Guard declines the tasks of ${listNames(signedIn)} at ${host}. ${missing}, `
    + `so leave or decline the project there for ${notSignedIn.length === 1 ? 'that rig' : 'those rigs'} too.`;
}

/** What telling the coordinator did, after leaving. */
export function coordinatorResult(told: DirectorCoordinatorOutcome): string {
  const declined = told.declined.length;
  if (told.problems.length === 0) {
    if (declined === 0) return `${told.host} had no task for this plan to decline.`;
    return declined === 1
      ? `${told.host} declined the plan's task.`
      : `${told.host} declined all ${declined} of the plan's tasks.`;
  }
  if (declined === 0) {
    return `${told.host} was not told: ${told.problems.join(' ')} Leave or decline the project on ${told.host} too.`;
  }
  return `${told.host} declined ${declined} of the plan's ${told.tasks} tasks. ${told.problems.join(' ')} `
    + `Leave or decline the project on ${told.host} for the rest.`;
}

/** What leaving did, for the dialog and the Activation section's status. */
export function leftSummary(left: DirectorCollaborationLeft): string {
  return [
    `${left.name} left ${collaborationName(left.collaboration)}.`,
    'It is now a plan of your own.',
    left.assignments_ended > 0
      ? `Ended ${left.assignments_ended.toLocaleString()} open assignment${left.assignments_ended === 1 ? '' : 's'}.`
      : '',
    coordinatorResult(left.coordinator),
  ].filter(Boolean).join(' ');
}
