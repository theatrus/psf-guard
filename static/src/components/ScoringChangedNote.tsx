import { useViewScoring } from '../hooks/useScoringPreferences';
import { describeScoringChanges } from '../utils/scoring';

interface ScoringChangedNoteProps {
  /** Beside Stacks' Build: says in full that a build uses this scoring. */
  build?: boolean;
}

/**
 * Says that the view scores with its own values, set in Sequence's Scoring
 * control, rather than the person's defaults, on the views that use the
 * scoring but have no control for it: Images, the detail view and Stacks.
 * Reset returns the view to the defaults. Nothing shows while the view
 * scores as the defaults do.
 */
export default function ScoringChangedNote({ build = false }: ScoringChangedNoteProps) {
  const { scoring, defaults, changed, resetToDefault } = useViewScoring();
  if (!changed) return null;
  const changes = describeScoringChanges(scoring, defaults).join(', ');

  if (build) {
    return (
      <div className="scoring-changed-note scoring-changed-build is-set" role="note">
        <span>
          <strong>Scoring (changed).</strong> Builds here use this view's scoring, not
          your defaults: {changes}.
        </span>
        <button type="button" onClick={resetToDefault}>
          Reset to default
        </button>
      </div>
    );
  }

  return (
    <span
      className="scoring-changed-note is-set"
      title={`This view scores with its own values, not your defaults: ${changes}.`}
    >
      Scoring (changed)
      <span className="visually-hidden">: {changes}.</span>
      <button type="button" aria-label="Reset scoring to default" onClick={resetToDefault}>
        Reset
      </button>
    </span>
  );
}
