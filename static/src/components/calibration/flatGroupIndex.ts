import type {
  CalibrationSession,
  FlatChangeEvent,
  FlatFamily,
  FlatGroup,
  FlatGroupsRig,
  FlatSession,
  FlatSplit,
  FlatUnmeasuredSession,
} from '../../api/types';

/** Group colours cycle through this many tokens; neighbours never share one. */
export const GROUP_TONES = 4;

/** Where one of the Calibration page's flat sessions sits in its family's groups. */
export interface FlatSessionPlace {
  rig: FlatGroupsRig;
  family: FlatFamily;
  group: FlatGroup;
  /** 1 for the family's oldest group. */
  number: number;
  /** Groups in the family. */
  groups: number;
  /** 1 to {@link GROUP_TONES}: the group's colour. */
  tone: number;
  session: FlatSession;
}

/** A line drawn between sessions of the list: a split, or a change of the whole rig. */
export type FlatMark =
  | { kind: 'split'; key: string; rig: FlatGroupsRig; family: FlatFamily; split: FlatSplit; end: number }
  | { kind: 'event'; key: string; rig: FlatGroupsRig; event: FlatChangeEvent; end: number };

export interface FlatGroupIndex {
  place: (sessionId: string) => FlatSessionPlace | undefined;
  unmeasured: (sessionId: string) => (FlatUnmeasuredSession & { rig: FlatGroupsRig }) | undefined;
  /** A measured session by its response key. */
  response: (responseKey: string) => FlatSession | undefined;
  /** The marks to draw above each listed session, by its index in `sessions`. */
  marks: (sessions: CalibrationSession[], page: number) => Map<number, FlatMark[]>;
}

/** Every rig's sessions, groups, splits and changes, looked up by the page's own sessions. */
export function indexFlatGroups(rigs: FlatGroupsRig[]): FlatGroupIndex {
  const places = new Map<string, FlatSessionPlace>();
  const responses = new Map<string, FlatSession>();
  const unmeasured = new Map<string, FlatUnmeasuredSession & { rig: FlatGroupsRig }>();
  for (const rig of rigs) {
    for (const family of rig.families) {
      // Groups come newest first; number them from the oldest.
      family.groups.forEach((group, position) => {
        const number = family.groups.length - position;
        for (const session of group.sessions) {
          responses.set(session.response_key, session);
          const place: FlatSessionPlace = {
            rig,
            family,
            group,
            number,
            groups: family.groups.length,
            tone: ((number - 1) % GROUP_TONES) + 1,
            session,
          };
          for (const id of session.sessions ?? []) if (!places.has(id)) places.set(id, place);
        }
      });
    }
    for (const entry of rig.unmeasured) {
      for (const id of entry.sessions ?? []) if (!unmeasured.has(id)) unmeasured.set(id, { ...entry, rig });
    }
  }

  const lastOf = (session: CalibrationSession) => places.get(session.id)?.session.last ?? session.last_at;

  const marks = (sessions: CalibrationSession[], page: number) => {
    const out = new Map<number, FlatMark[]>();
    const put = (index: number, mark: FlatMark) => {
      // At the top of the first page there is nothing newer to set it apart from.
      if (index < 0 || (index === 0 && page === 0)) return;
      out.set(index, [...(out.get(index) ?? []), mark]);
    };
    for (const rig of rigs) {
      // A change on the camera side reaches every filter and angle of the
      // rig: it shows once, above the newest listed flat of the rig shot
      // before it, and its splits do not show again in each filter.
      const rigWide = new Set(rig.events.filter((event) => event.camera_side).map((event) => event.key));
      for (const event of rig.events) {
        if (!rigWide.has(event.key)) continue;
        const index = sessions.findIndex((session) => {
          if (session.kind !== 'flat' || session.rig_uuid !== rig.rig_uuid) return false;
          const last = lastOf(session);
          return last !== null && last <= event.start;
        });
        put(index, { kind: 'event', key: `event:${event.key}`, rig, event, end: event.end });
      }
      // Any other split shows above the newest listed session of its
      // family's older groups.
      for (const family of rig.families) {
        family.splits.forEach((split, position) => {
          if (split.event_key && rigWide.has(split.event_key)) return;
          const index = sessions.findIndex((session) => {
            const place = places.get(session.id);
            return place?.family === family && family.groups.indexOf(place.group) > position;
          });
          put(index, {
            kind: 'split',
            key: `split:${family.key}:${split.before_response}:${split.after_response}`,
            rig,
            family,
            split,
            end: split.end,
          });
        });
      }
    }
    for (const list of out.values()) list.sort((left, right) => right.end - left.end);
    return out;
  };

  return {
    place: (id) => places.get(id),
    unmeasured: (id) => unmeasured.get(id),
    response: (key) => responses.get(key),
    marks,
  };
}
