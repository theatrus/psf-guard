import { useLayoutEffect, useRef } from 'react';
import { X } from 'lucide-react';
import type { Image } from '../api/types';
import type { SequenceIssue } from '../utils/sequenceIssues';
import { frameTime } from '../utils/frameTime';

const SEQUENCE_ISSUE_LIST_ID = 'sequence-issue-list';

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

/** Where focus goes when a list closes: the pill that opened it, else
 *  another pill, else the summary that holds them. */
function focusAfterList(key: SequenceIssue['key']) {
  (document.querySelector<HTMLElement>(`.summary-issues [data-issue="${key}"]`)
    ?? document.querySelector<HTMLElement>('.summary-issues [data-issue]')
    ?? document.querySelector<HTMLElement>('.sequence-summary-bar'))?.focus();
}

/** The pills: each opens the list of its frames, and closes it again. */
export function SequenceIssuePills({ issues, open, onToggle }: {
  issues: readonly SequenceIssue[];
  open: SequenceIssue['key'] | null;
  onToggle: (key: SequenceIssue['key']) => void;
}) {
  return (
    <div className="summary-issues">
      {issues.map(issue => (
        <button
          key={issue.key}
          type="button"
          className={`issue-badge ${issue.tone}`}
          aria-expanded={open === issue.key}
          aria-controls={open === issue.key ? SEQUENCE_ISSUE_LIST_ID : undefined}
          data-issue={issue.key}
          title={`List the ${plural(issue.frames.length, 'frame', 'frames')} and go to one`}
          onClick={() => onToggle(issue.key)}
          onKeyDown={(event) => {
            // Escape closes the pill's open list.
            if (event.key !== 'Escape' || open !== issue.key) return;
            event.preventDefault();
            onToggle(issue.key);
          }}
        >
          {issue.label}
        </button>
      ))}
    </div>
  );
}

/** An open pill's frames, in the strip's order. A click goes to one;
 *  × or Escape closes it and hands focus back to the pill. */
export function SequenceIssueList({ issue, imageMap, currentImageId, onGo, onClose }: {
  issue: SequenceIssue;
  imageMap: ReadonlyMap<number, Image>;
  currentImageId: number | null;
  onGo: (imageId: number) => void;
  onClose: () => void;
}) {
  const sectionRef = useRef<HTMLElement>(null);
  const close = () => {
    onClose();
    focusAfterList(issue.key);
  };
  // Gone with focus inside, as when another session tab or rescoring
  // takes its pill away, focus would fall to the page: it goes back to
  // the pills once the view has drawn.
  useLayoutEffect(() => {
    const section = sectionRef.current;
    return () => {
      if (!section?.contains(document.activeElement)) return;
      queueMicrotask(() => {
        const active = document.activeElement;
        if (active && active !== document.body && active.isConnected) return;
        focusAfterList(issue.key);
      });
    };
  }, [issue.key]);
  return (
    <section
      ref={sectionRef}
      id={SEQUENCE_ISSUE_LIST_ID}
      className="sequence-issue-list"
      aria-label={issue.title}
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return;
        event.preventDefault();
        event.stopPropagation();
        close();
      }}
    >
      <header>
        <span>
          <strong>{issue.title}</strong> · {plural(issue.frames.length, 'frame', 'frames')}
        </span>
        <button
          type="button"
          className="tool-button tool-button-quiet"
          aria-label="Close the list"
          onClick={close}
        >
          <X size={14} aria-hidden="true" />
        </button>
      </header>
      <ol>
        {issue.frames.map(frame => {
          const image = imageMap.get(frame.image_id);
          const name = image?.metadata?.FileName;
          const time = frameTime(image);
          const score = Math.round(frame.quality_score * 100);
          return (
            <li key={frame.image_id}>
              <button
                type="button"
                className="sequence-issue-frame"
                aria-current={frame.image_id === currentImageId ? 'true' : undefined}
                title={typeof name === 'string' ? name : undefined}
                aria-label={`${time}, score ${score}`}
                onClick={() => onGo(frame.image_id)}
              >
                {time} · {score}
              </button>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
