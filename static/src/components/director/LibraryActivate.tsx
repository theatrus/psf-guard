import { createContext, useContext, useState, type ReactNode } from 'react';
import { Send } from 'lucide-react';
import type { DirectorPlanRow } from '../../api/directorTypes';
import { useAccess } from '../../auth/access';
import { useDirectorStatus } from '../../hooks/useDirectorStatus';
import Dialog from '../Dialog';
import ActivationPanel from './ActivationPanel';
import { activationDueText } from './planCardModel';
import './DirectorPage.css';

/** A pill on a plan whose rigs lack its saved work; its title says why. */
export function NeedsActivationPill({ row }: { row: DirectorPlanRow }) {
  const reason = activationDueText(row);
  if (!reason) return null;
  return <span className="library-pill library-pill-due" title={reason}>Needs activating</span>;
}

type Open = (plan: { id: string; name: string }) => void;
const ActivationDialog = createContext<Open | null>(null);

/** The Library's activation dialog, one for the page. A row's button opens
 *  it; it stays open after Activate even when the plan, now on its rigs,
 *  leaves a Needs activating list, so the result can be read. */
export function LibraryActivationHost({ children }: { children: ReactNode }) {
  const [plan, setPlan] = useState<{ id: string; name: string } | null>(null);
  return <ActivationDialog.Provider value={setPlan}>
    {children}
    <Dialog open={!!plan} title={plan ? `Activate ${plan.name} on the rigs` : ''} onClose={() => setPlan(null)} className="activation-dialog">
      {/* The dialog sits outside the page; this keeps the planning styles. */}
      {plan && <div className="director-page director-embedded"><ActivationPanel key={plan.id} projectId={plan.id} /></div>}
    </Dialog>
  </ActivationDialog.Provider>;
}

/** Activate a plan from the Library. The button shows on a plan whose rigs
 *  lack its saved work, for an editor on a server that writes rig
 *  databases, inside a {@link LibraryActivationHost}. It opens the
 *  workspace's activation dialog: a preview of each rig's changes, then
 *  Activate to write them. */
export default function ActivateButton({ row, name, variant = 'icon' }: {
  row: DirectorPlanRow | null;
  name: string;
  /** `icon` for a compact row, `text` for a card's button strip. */
  variant?: 'icon' | 'text';
}) {
  const { canWrite } = useAccess();
  const status = useDirectorStatus();
  const open = useContext(ActivationDialog);
  const reason = row ? activationDueText(row) : null;
  if (!row || !reason || !open || !canWrite || !status.data?.database_management) return null;
  const title = `${reason}. Opens a preview of each rig's changes before anything is written.`;
  const onClick = (event: React.MouseEvent) => { event.stopPropagation(); open({ id: row.project.id, name }); };
  return variant === 'icon'
    ? <button type="button" className="library-planning library-activate" title={title} aria-label={`Activate the ${name} plan`} onClick={onClick}>
        <Send size={14} aria-hidden="true" />
      </button>
    : <button type="button" className="project-settings-button library-activate" title={title} aria-label={`Activate the ${name} plan`} onClick={onClick}>
        <Send size={14} aria-hidden="true" />Activate…
      </button>;
}
