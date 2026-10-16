import type { TimeMarks } from '../utils/timeBreaks';

/** A card's share of the time marks: the band behind every other night,
 *  and the label at the top of a break's line, the night's date or the
 *  hour. Render it before the card, so the card paints over the band. */
export default function TimeBreakMark({ marks, index }: { marks: TimeMarks | null; index: number }) {
  const mark = marks?.breaks.get(index);
  const band = marks?.altNight.has(index) ?? false;
  if (!mark && !band) return null;
  return (
    <>
      {band && <span className="time-night-band" aria-hidden="true" />}
      {mark && (
        <span className="time-break-label" data-kind={mark.kind} title={mark.title}>
          {mark.label}
        </span>
      )}
    </>
  );
}
