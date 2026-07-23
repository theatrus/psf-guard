import { useState } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import Dialog from '../Dialog';

function DialogHarness() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>Open review</button>
      <Dialog
        open={open}
        title="Review selection"
        onClose={() => setOpen(false)}
        footer={<button type="button">Confirm</button>}
      >
        <button type="button">First action</button>
      </Dialog>
    </>
  );
}

describe('Dialog', () => {
  it('locks the page, closes with Escape, and restores focus', async () => {
    const user = userEvent.setup();
    render(<DialogHarness />);

    const trigger = screen.getByText('Open review');
    await user.click(trigger);

    expect(screen.getByRole('dialog', { name: 'Review selection' })).toBeInTheDocument();
    expect(document.body.style.overflow).toBe('hidden');
    expect(screen.getByRole('button', { name: 'Close dialog' })).toHaveFocus();

    await user.keyboard('{Escape}');

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(document.body.style.overflow).toBe('');
    expect(trigger).toHaveFocus();
  });

  it('keeps tab focus inside the dialog', async () => {
    const user = userEvent.setup();
    render(<DialogHarness />);

    await user.click(screen.getByText('Open review'));
    const close = screen.getByRole('button', { name: 'Close dialog' });
    const confirm = screen.getByText('Confirm');

    close.focus();
    await user.keyboard('{Shift>}{Tab}{/Shift}');
    expect(confirm).toHaveFocus();

    await user.tab();
    expect(close).toHaveFocus();
  });
});
