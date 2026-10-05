import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';

/** The edits a page holds until its save bar saves them: which sections have
 *  them, and how to save or drop each. The bar itself is in `pageDrafts`. */
export interface DraftSection {
  label: string;
  /** Saving order: the framing before the plan it is read with. */
  order: number;
  unsaved: boolean;
  /** What differs from the saved state, in a few words each. */
  changes?: string[];
  /** Needs a first save though nobody edited it (a framing taken from the
   *  catalog, a panel rig picked for it). Saved with the rest, never shown
   *  as an unsaved change. */
  pending?: boolean;
  /** Save the section's edits; resolve false when it could not, having
   *  shown why in the section itself. */
  save: () => Promise<boolean>;
  /** Drop the section's edits and show what is saved. */
  discard: () => void;
}

export interface Registered extends DraftSection { id: string }

export interface Drafts {
  sections: Registered[];
  unsaved: Registered[];
  register: (id: string, section: DraftSection | null) => void;
  /** Save every section with edits, in order, stopping at the first that
   *  cannot be saved. Resolves the label of that section, or null. */
  saveAll: () => Promise<string | null>;
  discardAll: () => void;
}

export const DraftContext = createContext<Drafts | null>(null);

/** The page's draft registry. Sections register through
 *  [`useDraftSection`]; the page renders [`SaveBar`] once. */
export function usePageDrafts(): Drafts {
  const [sections, setSections] = useState<Record<string, Registered>>({});
  const live = useRef(sections);
  live.current = sections;
  const register = useCallback((id: string, section: DraftSection | null) => {
    setSections(current => {
      if (!section) {
        if (!(id in current)) return current;
        const next = { ...current };
        delete next[id];
        return next;
      }
      const previous = current[id];
      if (previous && previous.unsaved === section.unsaved && previous.pending === section.pending && previous.label === section.label && previous.order === section.order
        && JSON.stringify(previous.changes ?? []) === JSON.stringify(section.changes ?? [])) return current;
      return { ...current, [id]: { ...section, id } };
    });
  }, []);
  const ordered = useMemo(() => Object.values(sections).sort((left, right) => left.order - right.order), [sections]);
  const unsaved = useMemo(() => ordered.filter(section => section.unsaved), [ordered]);
  const saveAll = useCallback(async () => {
    const pending = Object.values(live.current).filter(section => section.unsaved || section.pending).sort((left, right) => left.order - right.order);
    for (const section of pending) {
      let saved = false;
      try { saved = await section.save(); } catch { saved = false; }
      if (!saved) return section.label;
    }
    return null;
  }, []);
  const discardAll = useCallback(() => { Object.values(live.current).forEach(section => { if (section.unsaved) section.discard(); }); }, []);
  return { sections: ordered, unsaved, register, saveAll, discardAll };
}

/** The page's drafts, when the component sits on a page that keeps them. */
export function useDrafts(): Drafts | null {
  return useContext(DraftContext);
}

/** Register a section with the page's save bar. Returns true when the page
 *  manages saving, so the section can leave out a Save button of its own. */
export function useDraftSection(id: string, section: DraftSection): boolean {
  const drafts = useContext(DraftContext);
  const latest = useRef(section);
  latest.current = section;
  const register = drafts?.register;
  useEffect(() => {
    register?.(id, {
      label: section.label,
      order: section.order,
      unsaved: section.unsaved,
      pending: section.pending,
      changes: section.changes,
      save: () => latest.current.save(),
      discard: () => latest.current.discard(),
    });
    // The change list is compared by value: a fresh array each render must
    // not re-register.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [register, id, section.label, section.order, section.unsaved, section.pending, JSON.stringify(section.changes ?? [])]);
  useEffect(() => () => register?.(id, null), [register, id]);
  return !!drafts;
}

