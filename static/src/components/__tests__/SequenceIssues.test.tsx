import { describe, expect, it } from 'vitest';
import { useState } from 'react';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SequenceIssueList, SequenceIssuePills } from '../SequenceIssues';
import type { ImageQualityResult } from '../../api/types';
import type { SequenceIssue } from '../../utils/sequenceIssues';

const frame = (id: number) => ({ image_id: id, quality_score: 0.4 }) as ImageQualityResult;
const clouds: SequenceIssue = {
  key: 'clouds', label: '2 cloud events', title: 'Likely clouds', tone: 'clouds', frames: [frame(1), frame(2)],
};

/** The summary as Sequence draws it: the pills, and the open pill's list.
 *  `issues` can drop the pill, as another session tab or a rescore can. */
function Summary({ issues }: { issues: SequenceIssue[] }) {
  const [open, setOpen] = useState<SequenceIssue['key'] | null>(null);
  const openIssue = issues.find(issue => issue.key === open) ?? null;
  return (
    <div className="sequence-summary-bar" tabIndex={-1}>
      <SequenceIssuePills issues={issues} open={openIssue?.key ?? null} onToggle={key => setOpen(current => current === key ? null : key)} />
      {openIssue && (
        <SequenceIssueList
          issue={openIssue}
          imageMap={new Map()}
          currentImageId={null}
          onGo={() => {}}
          onClose={() => setOpen(null)}
        />
      )}
    </div>
  );
}

describe('SequenceIssueList', () => {
  it('closes on Escape from within the list and hands focus back to the pill', async () => {
    const user = userEvent.setup();
    render(<Summary issues={[clouds]} />);
    const pill = screen.getByRole('button', { name: '2 cloud events' });
    await user.click(pill);
    const list = screen.getByRole('region', { name: 'Likely clouds' });
    within(list).getAllByRole('button')[1].focus();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('region', { name: 'Likely clouds' })).not.toBeInTheDocument();
    expect(pill).toHaveFocus();
    expect(pill).toHaveAttribute('aria-expanded', 'false');
  });

  it('closes on Escape from its pill', async () => {
    const user = userEvent.setup();
    render(<Summary issues={[clouds]} />);
    const pill = screen.getByRole('button', { name: '2 cloud events' });
    await user.click(pill);
    expect(pill).toHaveFocus();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('region', { name: 'Likely clouds' })).not.toBeInTheDocument();
    expect(pill).toHaveFocus();
  });

  it('moves focus to the summary when the list goes with its pill while focus is inside', async () => {
    const user = userEvent.setup();
    const { rerender, container } = render(<Summary issues={[clouds]} />);
    await user.click(screen.getByRole('button', { name: '2 cloud events' }));
    within(screen.getByRole('region', { name: 'Likely clouds' })).getAllByRole('button')[1].focus();

    rerender(<Summary issues={[]} />);
    expect(screen.queryByRole('region', { name: 'Likely clouds' })).not.toBeInTheDocument();
    await waitFor(() => expect(container.querySelector('.sequence-summary-bar')).toHaveFocus());
  });
});
