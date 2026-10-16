import { Fragment } from 'react';
import type { FamilyEntry, GroupEdge, GroupEntry } from './flatGroupBrowse';
import { groupName, splitShortReason } from './flatGroupBrowse';
import { nightSpan, percent, plural } from './flatGroupLabels';
import type { GroupEditing } from './FlatGroupDetail';
import { handLabel } from './useFlatGroupChoices';

interface FlatGroupListProps {
  families: FamilyEntry[];
  selected: string | undefined;
  /** Name rigs beside their families: more than one is in view. */
  showRig: boolean;
  /** The project in scope, for the title of a neighbour outside it. */
  scopeName?: string;
  onSelect: (key: string) => void;
  /** Join and reset at an edge; absent for a viewer who may not. */
  edit?: GroupEditing;
}

/** What an edge says inline: "Change · new dust 1.6%", "Split · pattern 2.6%". */
function edgeText(edge: GroupEdge): string {
  if (edge.split?.by_hand === 'split') return 'Split by you';
  if (edge.split) return `${edge.event ? 'Change' : 'Split'} · ${splitShortReason(edge.split)}`;
  const event = edge.event!;
  const what = event.what.split(':')[0];
  return `Change · ${/dust/.test(what) ? `${what} ${percent(event.dust_pct)}` : `pattern ${percent(event.broad_pct)}`}`;
}

function EdgeRow({ edge, title, edit }: { edge: GroupEdge; title: string; edit?: GroupEditing }) {
  const split = edge.split;
  const hand = split ? handLabel(split) : null;
  const choice = split?.choice ?? null;
  return (
    <li className="flat-group-edge" data-kind={edge.event ? 'event' : 'split'} data-testid="flat-group-edge"
      data-hand={hand ? 'true' : undefined} title={title}>
      <span aria-hidden="true">↕</span> <span>{edgeText(edge)}</span>
      {hand && split?.by_hand !== 'split' && <span className="flat-group-hand-tag">{hand}</span>}
      {choice?.conflict && <span className="flat-group-hand-tag" data-conflict="true">conflict</span>}
      {edit && split && (
        <button type="button" className="calibration-link-button flat-group-edge-action"
          data-testid={choice ? 'flat-group-list-reset' : 'flat-group-list-join'} disabled={edit.busy}
          aria-label={choice
            ? `Reset the ${choice.choice === 'join' ? 'join' : 'split'} at ${split.after_night}`
            : `Join the groups either side of ${split.after_night}`}
          onClick={() => edit.send(choice
            ? { choice: 'reset', anchor_frame: choice.anchor_frame }
            : { choice: 'join', after_response: split.after_response, before_response: split.before_response })}>
          {choice ? 'Reset' : 'Join'}
        </button>
      )}
    </li>
  );
}

/** The marks a group carries in the list. */
export function GroupMarks({ entry, scopeName }: { entry: GroupEntry; scopeName?: string }) {
  const waiting = entry.unmeasured.filter((session) => session.reason === 'not measured yet').length;
  return (
    <span className="calibration-session-counts">
      {entry.current && (
        <span className="calibration-badge flat-group-mark" data-mark="current"
          title="A light shot now takes this group's flats">Current</span>
      )}
      {entry.unmeasured.length > 0 && (
        <span className="calibration-badge flat-group-mark" data-mark="partial"
          title={`${plural(entry.unmeasured.length, 'night')} of its flats ${waiting > 0 ? 'not measured yet' : 'too few to measure'}`}>
          Partly measured
        </span>
      )}
      {entry.joins.length > 0 && (
        <span className="calibration-badge flat-group-mark" data-mark="hand"
          title={`${plural(entry.joins.length, 'measured split')} joined by you inside it`}>Joined by you</span>
      )}
      {entry.newer?.event && (
        <span className="calibration-badge flat-group-mark" data-mark="change"
          title={`A change came after it: ${entry.newer.event.what}`}>Change after</span>
      )}
      {entry.context === 'neighbour' && (
        <span className="calibration-badge flat-group-mark" data-mark="context"
          title={scopeName ? `Not used by ${scopeName}: shown for the edge it shares` : 'Shown for the edge it shares'}>
          Neighbour
        </span>
      )}
      {entry.context === 'map' && (
        <span className="calibration-badge flat-group-mark" data-mark="context"
          title={`${scopeName ?? 'The project'}'s lights take none of its flats; a correction's map is measured from it`}>
          Correction map
        </span>
      )}
    </span>
  );
}

/**
 * Flat groups in Frames' sidebar, by rig and family, newest first, with the
 * split or change between neighbours drawn between them, so the list reads
 * as a timeline. Each group opens its detail.
 */
export default function FlatGroupList({ families, selected, showRig, scopeName, onSelect, edit }: FlatGroupListProps) {
  return (
    <nav className="calibration-sessions flat-group-list" aria-label="Flat groups">
      <ul>
        {families.map((family) => (
          <Fragment key={family.key}>
            <li className="flat-group-family" data-testid="flat-group-family">
              {showRig ? `${family.rig.name} · ` : ''}{family.label}
            </li>
            {family.groups.map((entry, index) => {
              const next = family.groups[index + 1];
              const skipped = next ? entry.number - next.number - 1 : 0;
              return (
                <Fragment key={entry.key}>
                  {index === 0 && entry.newer && !entry.newer.split && (
                    <EdgeRow edge={entry.newer} title={entry.newer.event?.what ?? ''} />
                  )}
                  <li data-flat-tone={entry.tone} data-context={entry.context || undefined}>
                    <button
                      type="button"
                      aria-current={entry.key === selected ? 'true' : undefined}
                      aria-label={groupName(entry)}
                      data-group-key={entry.key}
                      data-testid="flat-group-entry"
                      onClick={() => onSelect(entry.key)}
                    >
                      <span className="calibration-session-head">
                        <strong>Group {entry.number}</strong>
                        <span>{nightSpan(entry.group.first_night, entry.group.last_night)}</span>
                      </span>
                      <span className="calibration-muted">
                        {[plural(entry.group.sessions.length, 'night'), plural(entry.group.frames, 'flat')].join(' · ')}
                      </span>
                      <GroupMarks entry={entry} scopeName={scopeName} />
                    </button>
                  </li>
                  {next && skipped === 0 && entry.older && (
                    <EdgeRow edge={entry.older} title={entry.older.split?.reason ?? ''} edit={edit} />
                  )}
                  {next && skipped > 0 && (
                    <li className="flat-group-edge" data-kind="gap">
                      {plural(skipped, 'group')} {scopeName ? `not used by ${scopeName}` : 'not shown'}
                    </li>
                  )}
                </Fragment>
              );
            })}
          </Fragment>
        ))}
      </ul>
    </nav>
  );
}
