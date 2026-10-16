import type { TimeMarks } from '../utils/timeBreaks';

/** The label at the top of a break's line: the night's date or the hour. */
export default function TimeBreakMark({ marks, index }: { marks: TimeMarks | null; index: number }) {
  const mark = marks?.breaks.get(index);
  if (!mark) return null;
  return (
    <span className="time-break-label" data-kind={mark.kind} title={mark.title}>
      {mark.label}
    </span>
  );
}
