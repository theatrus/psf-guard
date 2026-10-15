import { RefreshCw, Send } from 'lucide-react';
import Dialog from '../Dialog';
import ActivationPanel from './ActivationPanel';
import { sendLabel, useActivationBar, useActivationState, useSendAgain } from './activationState';
import type { Drafts } from './pageDraftsState';

/** Activation as an action at the top of the page, beside the save bar:
 *  once everything is saved, it asks for an activation while the rig
 *  databases lack the saved plan or framing or their rows differ from it,
 *  and opens the preview. While a remote site lacks the last activation it
 *  says so and offers to send it there again. */
export default function ActivationBar({ projectId, drafts, canWrite, open, onOpen, onClose }: {
  projectId: string;
  drafts: Drafts;
  canWrite: boolean;
  open: boolean;
  onOpen: () => void;
  onClose: () => void;
}) {
  const { last, notSynced, notSent, unsent } = useActivationState(projectId);
  const { activate, send } = useActivationBar(projectId, canWrite, drafts.unsaved.length);
  const sendAgain = useSendAgain(projectId);
  return <>
    {(activate || send) && <div className="draft-bar is-due" role="region" aria-label="Rigs behind">
      <p className="draft-bar-message">{activate ? notSynced : notSent}</p>
      <div className="draft-bar-actions">
        {send && <button type="button" className={activate ? undefined : 'is-primary'} disabled={sendAgain.isPending}
          title={unsent.map(push => push.error).filter(Boolean).join(' ') || undefined} onClick={() => sendAgain.mutate()}>
          <RefreshCw size={16} />{sendAgain.isPending ? 'Sending…' : sendLabel(unsent)}
        </button>}
        {/* The summary's button hides while the bar is up, so the bar opens the dialog either way. */}
        <button type="button" className={activate ? 'is-primary' : undefined} onClick={onOpen}><Send size={16} />{activate && last.data ? 'Send to rigs again…' : 'Send to rigs…'}</button>
      </div>
    </div>}
    <Dialog open={open} title="Send to the rigs" onClose={onClose} className="activation-dialog">
      {/* The dialog sits outside the page; this keeps the page's styles. */}
      <div className="director-page director-embedded"><ActivationPanel projectId={projectId} /></div>
    </Dialog>
  </>;
}
