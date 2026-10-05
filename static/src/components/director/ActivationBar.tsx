import { Send } from 'lucide-react';
import Dialog from '../Dialog';
import ActivationPanel from './ActivationPanel';
import { useActivationState } from './activationState';
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
  const { last, savedPlan, behind } = useActivationState(projectId);
  const shoots = (savedPlan.data?.plan?.contributions.length ?? 0) > 0;
  const due = canWrite && last.isSuccess && shoots && (!last.data || behind.length > 0);
  // Unsaved edits come first: the save bar is showing, and activation
  // reads only what is saved.
  const show = due && drafts.unsaved.length === 0;
  return <>
    {show && <div className="draft-bar is-due" role="region" aria-label="Activation due">
      <p className="draft-bar-message">{last.data
        ? `The rig databases have activation revision ${last.data.revision}; the saved ${behind.join(' and ')} ${behind.length === 1 ? 'is' : 'are'} newer.`
        : 'Not activated yet: the rig databases have none of this plan.'}</p>
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
