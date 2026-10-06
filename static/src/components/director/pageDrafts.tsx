import { useContext, useEffect, useState, type ReactNode } from 'react';
import { UNSAFE_DataRouterContext, useBlocker, type Blocker } from 'react-router-dom';
import { Check, Undo2 } from 'lucide-react';
import { DraftContext, describeFailure, type Drafts, type SaveFailure } from './pageDraftsState';
import './pageDrafts.css';

export function DraftProvider({ drafts, children }: { drafts: Drafts; children: ReactNode }) {
  return <DraftContext.Provider value={drafts}>{children}</DraftContext.Provider>;
}

/** A section's changes the bar lists before "and N more". */
const MAX_CHANGES_SHOWN = 4;

const listed = (labels: string[]) => labels.length <= 1 ? labels.join('') : `${labels.slice(0, -1).join(', ')} and ${labels[labels.length - 1]}`;

/** The one place a page's edits are saved or dropped. It stays on screen
 *  while anything is unsaved, and asks before the page is left with edits
 *  in it, by a link in the app or by closing or reloading the tab. */
export function SaveBar({ drafts, canWrite }: { drafts: Drafts; canWrite: boolean }) {
  // Leaving by a link can only be held under a data router, which the app
  // uses; a component rendered on its own still warns before the tab closes.
  return useContext(UNSAFE_DataRouterContext)
    ? <GuardedSaveBar drafts={drafts} canWrite={canWrite} />
    : <SaveBarView drafts={drafts} canWrite={canWrite} blocker={null} />;
}

function GuardedSaveBar({ drafts, canWrite }: { drafts: Drafts; canWrite: boolean }) {
  const dirty = canWrite && drafts.unsaved.length > 0;
  const blocker = useBlocker(({ currentLocation, nextLocation }) => dirty && currentLocation.pathname !== nextLocation.pathname);
  return <SaveBarView drafts={drafts} canWrite={canWrite} blocker={blocker} />;
}

function SaveBarView({ drafts, canWrite, blocker }: { drafts: Drafts; canWrite: boolean; blocker: Blocker | null }) {
  const [state, setState] = useState<{ kind: 'idle' | 'saving' | 'saved' } | { kind: 'failed'; failure: SaveFailure }>({ kind: 'idle' });
  const labels = drafts.unsaved.map(section => section.label);
  const dirty = canWrite && labels.length > 0;
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);
  useEffect(() => {
    if (state.kind !== 'saved') return;
    const timer = window.setTimeout(() => setState({ kind: 'idle' }), 4000);
    return () => window.clearTimeout(timer);
  }, [state.kind]);
  const save = async () => {
    setState({ kind: 'saving' });
    const failed = await drafts.saveAll();
    setState(failed ? { kind: 'failed', failure: failed } : { kind: 'saved' });
    return !failed;
  };
  const discard = () => {
    if (!window.confirm(`Discard the unsaved changes in ${listed(labels)}?`)) return;
    drafts.discardAll();
    setState({ kind: 'idle' });
  };
  if (!dirty && state.kind !== 'saved' && blocker?.state !== 'blocked') return null;
  const leaving = blocker?.state === 'blocked';
  return <div className={`draft-bar${dirty ? ' is-dirty' : ''}`} role="region" aria-label="Unsaved changes">
    {leaving
      ? <p className="draft-bar-message" role="alert">Leave with unsaved changes in {listed(labels)}?</p>
      : dirty
        ? <p className="draft-bar-message">{state.kind === 'failed' ? describeFailure(state.failure) : `Unsaved changes in ${listed(labels)}.`}</p>
        : <p className="draft-bar-message" role="status"><Check size={16} />All changes saved.</p>}
    {dirty && drafts.unsaved.some(section => (section.changes ?? []).length > 0) && <ul className="draft-bar-changes" aria-label="What changed">
      {drafts.unsaved.filter(section => (section.changes ?? []).length > 0).map(section => {
        const changes = section.changes ?? [];
        const shown = changes.slice(0, MAX_CHANGES_SHOWN);
        const text = `${shown.join('; ')}${changes.length > shown.length ? `; and ${changes.length - shown.length} more` : ''}`;
        return <li key={section.id} title={`${section.label}: ${changes.join('; ')}`}><strong>{section.label}:</strong> {text}</li>;
      })}
    </ul>}
    {dirty && <div className="draft-bar-actions">
      {leaving
        ? <>
          <button type="button" onClick={() => blocker?.reset?.()}>Stay</button>
          <button type="button" onClick={() => blocker?.proceed?.()}>Leave without saving</button>
          <button type="button" className="is-primary" disabled={state.kind === 'saving'} onClick={async () => { if (await save()) blocker?.proceed?.(); else blocker?.reset?.(); }}>Save and leave</button>
        </>
        : <>
          <button type="button" disabled={state.kind === 'saving'} onClick={discard}><Undo2 size={16} />Discard changes</button>
          <button type="button" className="is-primary" disabled={state.kind === 'saving'} onClick={() => void save()}><Check size={16} />{state.kind === 'saving' ? 'Saving...' : 'Save changes'}</button>
        </>}
    </div>}
  </div>;
}

/** "edited" beside a section's heading while it holds unsaved edits. */
export function EditedMark({ drafts, id }: { drafts: Drafts; id: string }) {
  return drafts.unsaved.some(section => section.id === id) ? <span className="draft-edited">edited</span> : null;
}
