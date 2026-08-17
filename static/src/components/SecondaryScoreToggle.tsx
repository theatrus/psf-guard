import {
  setDisplayPreferences,
  useDisplayPreferences,
} from '../hooks/useDisplayPreferences';
import type { QualityScoreScope } from '../utils/qualityScore';

/**
 * Quick toggle for the second score chip. Each view shows one chip type —
 * the basis its main badge is NOT using — so the box binds to that type
 * and flipping it always changes the cards in front of the user. The
 * per-type choice persists, shared by every view that shows that type.
 */
export default function SecondaryScoreToggle({
  chipScope,
  className,
}: {
  /** The comparison basis the chips in this view carry. */
  chipScope: QualityScoreScope;
  className?: string;
}) {
  const preferences = useDisplayPreferences();
  const checked =
    chipScope === 'capture_sequence'
      ? preferences.showNightChip
      : preferences.showAllChip;
  const title =
    chipScope === 'capture_sequence'
      ? 'Show a second smaller chip with the score relative to the frame’s own session'
      : 'Show a second smaller chip with the score relative to every stack candidate for the filter';
  return (
    <label
      className={`secondary-score-toggle${className ? ` ${className}` : ''}`}
      title={title}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) =>
          setDisplayPreferences(
            chipScope === 'capture_sequence'
              ? { ...preferences, showNightChip: event.target.checked }
              : { ...preferences, showAllChip: event.target.checked },
          )
        }
      />
      2nd score
    </label>
  );
}
