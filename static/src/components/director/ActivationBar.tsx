import { Send } from 'lucide-react';
import Dialog from '../Dialog';
import ActivationPanel from './ActivationPanel';
import { useActivationDue, useActivationState } from './activationState';
import type { Drafts } from './pageDraftsState';

/** Activation as an action at the top of the page, beside the save bar:
 *  once everything is saved, it asks for an activation while the rig
 *  databases lack the saved plan or framing, and opens the preview. */
export default function ActivationBar({ projectId, drafts, canWrite, open, onOpen, onClose }: {
  projectId: string;
  drafts: Drafts;
  canWrite: boolean;
  open: boolean;
  onOpen: () => void;
  onClose: () => void;
}) {
  const { last, behind } = useActivationState(projectId);
  const show = useActivationDue(projectId, canWrite, drafts.unsaved.length);
  return <>
    {show && <div className="draft-bar is-due" role="region" aria-label="Activation due">
      <p className="draft-bar-message">{last.data
        ? `Saved ${behind.join(' and ')} not on the rigs yet`
        : 'Not activated yet'}</p>
      <div className="draft-bar-actions">
        <button type="button" className="is-primary" onClick={onOpen}><Send size={16} />Activate…</button>
      </div>
    </div>}
    <Dialog open={open} title="Activate on the rigs" onClose={onClose} className="activation-dialog">
      {/* The dialog sits outside the page; this keeps the page's styles. */}
      <div className="director-page director-embedded"><ActivationPanel projectId={projectId} /></div>
    </Dialog>
  </>;
}
