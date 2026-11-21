import { Plus } from 'lucide-react';

/** Why no rig can join a plan with no objectives, with the way out: Add
 *  objective adds one and opens Exposures. Rigs and Framing both show it. */
export default function NeedsObjectiveNote({ onAdd }: { onAdd: () => void }) {
  return (
    <p className="plan-needs-objective" role="note">
      A rig joins a plan by shooting its objectives, and this plan has none yet.{' '}
      <button type="button" onClick={onAdd}><Plus size={16} />Add objective</button>
    </p>
  );
}
