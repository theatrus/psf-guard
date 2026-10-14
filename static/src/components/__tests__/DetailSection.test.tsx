import { useState } from 'react';
import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import DetailSection from '../DetailSection';

function Section({ initiallyOpen = true }: { initiallyOpen?: boolean }) {
  const [open, setOpen] = useState(initiallyOpen);
  return (
    <DetailSection
      title="Sky context"
      badge={<span>Hinted solve</span>}
      summary="solved · 3 objects"
      open={open}
      onToggle={() => setOpen(!open)}
    >
      <p>Center 08h 40m</p>
    </DetailSection>
  );
}

describe('DetailSection', () => {
  it('folds and opens from its heading, which says so', () => {
    render(<Section />);
    const toggle = screen.getByRole('button', { name: 'Sky context' });
    expect(screen.getByRole('heading', { name: 'Sky context' })).toContainElement(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    const body = document.getElementById(toggle.getAttribute('aria-controls')!);
    expect(body).toContainElement(screen.getByText('Center 08h 40m'));

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    // The body is gone, so a folded panel renders and asks for nothing.
    expect(screen.queryByText('Center 08h 40m')).toBeNull();
    expect(body).not.toBeVisible();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('Center 08h 40m')).toBeVisible();
  });

  it('shows its summary only while folded, and its badge either way', () => {
    render(<Section initiallyOpen={false} />);
    const toggle = screen.getByRole('button', { name: 'Sky context solved · 3 objects' });
    expect(screen.getByText('Hinted solve')).toBeInTheDocument();
    // Cut short with an ellipsis on a narrow sidebar, so the whole line is its title.
    expect(document.querySelector('.detail-section-summary')).toHaveAttribute('title', 'solved · 3 objects');

    fireEvent.click(toggle);
    expect(toggle).toHaveAccessibleName('Sky context');
    expect(document.querySelector('.detail-section-summary')).toBeNull();
    expect(screen.getByText('Hinted solve')).toBeInTheDocument();
  });

  it('takes Enter and Space like any button', async () => {
    const user = userEvent.setup();
    render(<Section />);
    const toggle = screen.getByRole('button', { name: 'Sky context' });
    toggle.focus();
    await user.keyboard('{Enter}');
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await user.keyboard(' ');
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
  });

  it('is a plain heading with nothing to fold it', () => {
    render(
      <DetailSection title="Quality analysis" open>
        <p>36%</p>
      </DetailSection>,
    );
    expect(screen.getByRole('heading', { name: 'Quality analysis' })).toBeInTheDocument();
    expect(screen.queryByRole('button')).toBeNull();
    expect(screen.getByText('36%')).toBeInTheDocument();
  });
});
