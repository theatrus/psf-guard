import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { FlatGroupsRig, FlatSession, FlatSplit } from '../../../api/types';
import { browseGroups } from '../flatGroupBrowse';
import { windowLabel } from '../flatGroupLabels';
import FlatGroupDetail, { type GroupEditing } from '../FlatGroupDetail';
import FlatGroupList from '../FlatGroupList';

const session = (night: string, response: string): FlatSession => ({
  response_key: response,
  night,
  first: Date.parse(`${night}T12:00:00Z`) / 1000,
  last: Date.parse(`${night}T12:10:00Z`) / 1000,
  frames: 10,
  combined: 10,
  noise_pct: 0.04,
  bin: 8,
  width: 100,
  height: 80,
  previous: null,
  sessions: [`s-${response}`],
});

const handSplit: FlatSplit = {
  before_response: 'r2',
  after_response: 'r3',
  before_night: '2026-09-12',
  after_night: '2026-09-20',
  start: 1,
  end: 2,
  dust_pct: 0.2,
  broad_pct: 0.3,
  reason: 'split by you; measured alike: 0.2% dust (limit 1%), 0.3% light pattern (limit 2%)',
  event_key: null,
  spots: [],
  by_hand: 'split',
  choice: {
    choice: 'split', label: 'HA at 0.0°, 2026-09-20 starts a group', chosen_at: 1_789_000_000,
    anchor_frame: 'flat-r3', agrees: false, orphaned: null, conflict: null,
  },
};

const joined: FlatSplit = {
  before_response: 'r1',
  after_response: 'r2',
  before_night: '2026-09-10',
  after_night: '2026-09-12',
  start: 1,
  end: 2,
  dust_pct: 1.6,
  broad_pct: 0.4,
  reason: 'joined by you; measured apart: 1.6% dust (limit 1%), 0.4% light pattern (limit 2%)',
  event_key: 'c-1',
  spots: [],
  by_hand: 'joined',
  choice: {
    choice: 'join', label: 'HA at 0.0°, 2026-09-12 joined with 2026-09-10', chosen_at: 1_789_000_000,
    anchor_frame: 'flat-r2', agrees: false, orphaned: null,
    conflict: { key: 'c-1', start: 1, end: 2, what: 'new dust: 1 shadow near (1423, 3545)', filters: ['HA', 'OIII'] },
  },
};

const rig: FlatGroupsRig = {
  rig_uuid: 'rig',
  name: 'C925',
  measured: true,
  dust_limit_pct: 1,
  pattern_limit_pct: 2,
  angle_term: null,
  angle_pairs: 0,
  events: [],
  unmeasured: [],
  orphaned: [{
    choice: 'split', label: 'HA, 2026-08-01 starts a group', chosen_at: 1, anchor_frame: 'gone',
    agrees: null, orphaned: 'the flats it was made at are gone', conflict: null,
  }],
  families: [{
    key: 'fam',
    filter: 'HA',
    rotation: 0,
    groups: [
      { key: 'g2', first_night: '2026-09-20', last_night: '2026-09-20', frames: 10, sessions: [session('2026-09-20', 'r3')] },
      {
        key: 'g1', first_night: '2026-09-10', last_night: '2026-09-12', frames: 20,
        sessions: [session('2026-09-12', 'r2'), session('2026-09-10', 'r1')],
      },
    ],
    splits: [handSplit],
    joins: [joined],
  }],
};

function detail(key: string, edit?: GroupEditing) {
  const entry = browseGroups([rig]).find(key)!;
  render(
    <MemoryRouter>
      <FlatGroupDetail dbId="main" entry={entry} use={undefined} useError={null} canManage
        open={null} onCompare={() => {}} onOpenSession={() => {}} onShowNight={() => {}}
        shownSession={null} opening={null} edit={edit} />
    </MemoryRouter>,
  );
}

const editing = () => ({ send: vi.fn(), busy: false, error: null });

describe('splits and joins by hand', () => {
  it('draws a split by hand apart from measured ones, with its numbers and a Reset', () => {
    const edit = editing();
    detail('g1', edit);
    const edge = screen.getByTestId('flat-group-newer-edge');
    expect(edge).toHaveAttribute('data-hand', 'true');
    expect(within(edge).getByTestId('flat-group-hand')).toHaveTextContent('Split by you');
    expect(edge).toHaveTextContent('0.2% dust (limit 1%)');
    fireEvent.click(within(edge).getByRole('button', { name: 'Reset' }));
    expect(edit.send).toHaveBeenCalledWith({ choice: 'reset', anchor_frame: 'flat-r3' });
  });

  it('shows a join with the measured split it overrides and the camera-side change it stands against', () => {
    detail('g1', editing());
    const join = screen.getByTestId('flat-group-join-note');
    expect(join).toHaveTextContent('Measured 1.6% dust (limit 1%)');
    expect(within(join).getByTestId('flat-group-conflict')).toHaveTextContent('seen in HA, OIII');
  });

  it('splits at a member night, never at the group’s first', () => {
    const edit = editing();
    detail('g1', edit);
    const buttons = screen.getAllByTestId('flat-group-split-here');
    expect(buttons).toHaveLength(1);
    fireEvent.click(buttons[0]);
    expect(edit.send).toHaveBeenCalledWith({ choice: 'split', after_response: 'r2' });
  });

  it('lists choices that no longer apply under their rig', () => {
    detail('g1', editing());
    expect(screen.getByTestId('flat-group-orphaned')).toHaveTextContent('the flats it was made at are gone');
  });

  it('joins at a measured split from the list, and marks a group that holds a join', () => {
    const measured: FlatSplit = { ...handSplit, reason: 'new dust: 1 shadow near (1, 1), 1.6% of the background (limit 1%)', dust_pct: 1.6, by_hand: null, choice: null };
    const view: FlatGroupsRig = { ...rig, families: [{ ...rig.families[0], splits: [measured] }] };
    const edit = editing();
    const browse = browseGroups([view]);
    render(<FlatGroupList families={browse.families} selected={undefined} showRig={false} onSelect={() => {}} edit={edit} />);
    fireEvent.click(screen.getByTestId('flat-group-list-join'));
    expect(edit.send).toHaveBeenCalledWith({ choice: 'join', after_response: 'r3', before_response: 'r2' });
    expect(screen.getAllByText('Joined by you').length).toBeGreaterThan(0);
  });

  it('dates a split by hand at one moment on the rig’s clock', () => {
    // 2026-09-28 20:00 UTC, on a rig whose frames record UTC−7.
    const at = Date.parse('2026-09-28T20:00:00Z') / 1000;
    expect(windowLabel(at, at, [{ from: 0, offset_seconds: -7 * 3600 }])).toBe('at 2026-09-28 13:00 UTC−7');
    expect(windowLabel(at, at + 3600, [{ from: 0, offset_seconds: -7 * 3600 }]))
      .toBe('2026-09-28 13:00 – 2026-09-28 14:00 UTC−7');
  });

  it('shows a viewer the choices without a control to change them', () => {
    detail('g1');
    expect(screen.getByTestId('flat-group-hand')).toHaveTextContent('Split by you');
    expect(screen.queryByTestId('flat-group-split-here')).toBeNull();
    expect(screen.queryByTestId('flat-group-reset')).toBeNull();
    expect(screen.queryByTestId('flat-group-join')).toBeNull();
  });
});
