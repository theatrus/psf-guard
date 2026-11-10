import { describe, expect, it } from 'vitest';
import type { DirectorPlanLink, DirectorPlanRow } from '../../../api/directorTypes';
import { plansWaitingForFrames, shownWaitingPlans, waitingPlans } from '../waitingPlans';

const rig = { id: 'r', name: 'r', revision: 1 };
const link = (catalog_slug: string, source_row_id: number | null, source_state: number | null = 1, extra: Partial<DirectorPlanLink> = {}): DirectorPlanLink => ({
  catalog_slug, catalog_name: catalog_slug === 'askar' ? 'Askar' : 'Ultracat', rig, source_project_guid: `${catalog_slug}-g`, source_row_id,
  source_name: 'Propeller', source_state, earliest_capture_s: null, latest_capture_s: null, targets: [], ...extra,
});
const plan = (name: string, links: DirectorPlanLink[]): DirectorPlanRow => ({ project: { id: name, name, revision: 1 }, links, progress: null, framing: null, plan: null, activation: null });
const names = (rows: DirectorPlanRow[]) => rows.map(row => row.project.name);
// Askar has frames for project 6; Ultracat has frames for nothing.
const listed = new Set(['askar:6']);

describe('plans waiting for frames', () => {
  it("judges a filtered Library by that database's row and project state", () => {
    const active = plan('Propeller', [link('askar', 6), link('ultracat', 20)]);
    const closedThere = plan('Closed there', [link('askar', 6), link('ultracat', 21, 3)]);
    const draftThere = plan('Draft there', [link('askar', 6), link('ultracat', 22, 0)]);
    const rows = [active, closedThere, draftThere];
    const waiting = waitingPlans(rows, listed, false, 'ultracat');
    expect(names(waiting)).toEqual(['Propeller', 'Closed there', 'Draft there']);
    // Show reads the Ultracat project's state, not Askar's.
    expect(names(shownWaitingPlans(waiting, 'all', '', 'ultracat'))).toEqual(['Propeller', 'Draft there']);
    expect(names(shownWaitingPlans(waiting, 'active', '', 'ultracat'))).toEqual(['Propeller']);
    expect(names(shownWaitingPlans(waiting, 'closed', '', 'ultracat'))).toEqual(['Closed there']);
    // Without the filter the Askar rows stand for all three.
    expect(names(waitingPlans(rows, listed, false))).toEqual([]);
  });

  it('does not take a database the server could not read for one with no frames', () => {
    const unread = plan('Unread', [link('ultracat', null, null, { source_unread: true })]);
    const partly = plan('Partly', [link('askar', 7), link('ultracat', null, null, { source_unread: true })]);
    expect(names(waitingPlans([unread], listed, false))).toEqual([]);
    expect(names(waitingPlans([partly], listed, false, 'ultracat'))).toEqual([]);
    expect(plansWaitingForFrames([unread], listed, false, '')).toEqual([]);
    // Askar's project 7 has no frames; only that rig is listed.
    expect(plansWaitingForFrames([partly], listed, false, '').map(({ rigs }) => rigs.map(entry => entry.catalog_name))).toEqual([['Askar']]);
  });

  it('offers the picker plans on no rig and plans with a rig still to capture, found by rig name too', () => {
    const rows = [plan('Unlinked', []), plan('Propeller', [link('askar', 6), link('ultracat', 20)]), plan('Done', [link('askar', 6)]), plan('Closed', [link('ultracat', 21, 3)])];
    expect(plansWaitingForFrames(rows, listed, false, '').map(({ row, rigs }) => [row.project.name, rigs.map(entry => entry.catalog_name)]))
      .toEqual([['Unlinked', []], ['Propeller', ['Ultracat']]]);
    expect(names(plansWaitingForFrames(rows, listed, false, 'ultracat').map(({ row }) => row))).toEqual(['Propeller']);
    // A database that could not be listed leaves only plans on no rig.
    expect(names(plansWaitingForFrames(rows, listed, true, '').map(({ row }) => row))).toEqual(['Unlinked']);
  });
});
