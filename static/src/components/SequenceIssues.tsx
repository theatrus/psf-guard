import { X } from 'lucide-react';
import type { Image } from '../api/types';
import type { SequenceIssue } from '../utils/sequenceIssues';

const SEQUENCE_ISSUE_LIST_ID = 'sequence-issue-list';

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

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
        >
          {issue.label}
        </button>
      ))}
    </div>
  );
}

function frameTime(image: Image | undefined): string {
  if (!image?.acquired_date) return 'No time';
  return new Date(image.acquired_date * 1000).toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/** An open pill's frames, in the strip's order. A click goes to one. */
export function SequenceIssueList({ issue, imageMap, currentImageId, onGo, onClose }: {
  issue: SequenceIssue;
  imageMap: ReadonlyMap<number, Image>;
  currentImageId: number | null;
  onGo: (imageId: number) => void;
  onClose: () => void;
}) {
  return (
    <section id={SEQUENCE_ISSUE_LIST_ID} className="sequence-issue-list" aria-label={issue.title}>
      <header>
        <span>
          <strong>{issue.title}</strong> · {plural(issue.frames.length, 'frame', 'frames')}
        </span>
        <button
          type="button"
          className="tool-button tool-button-quiet"
          aria-label="Close the list"
          onClick={() => {
            onClose();
            // Focus goes back to the pill that opened the list.
            document.querySelector<HTMLElement>(`.summary-issues [data-issue="${issue.key}"]`)?.focus();
          }}
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
