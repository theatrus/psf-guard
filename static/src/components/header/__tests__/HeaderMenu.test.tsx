import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import HeaderMenu from '../HeaderMenu';

describe('HeaderMenu', () => {
  it('holds Settings, shortcuts, server information and sign out behind one button', () => {
    const settings = vi.fn();
    const signOut = vi.fn();
    render(<HeaderMenu onSettings={settings} onShortcuts={vi.fn()} onServerInfo={vi.fn()} signedInAs="ada" onSignOut={signOut} />);
    const button = screen.getByRole('button', { name: 'Menu' });
    expect(button).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(button);
    expect(screen.getByText('Signed in as ada')).toBeInTheDocument();
    expect(screen.getAllByRole('menuitem').map((item) => item.textContent?.trim())).toEqual([
      'Settings', 'Keyboard shortcuts', 'Server information', 'Sign out',
    ]);
    expect(screen.getByRole('menuitem', { name: 'Settings' })).toHaveFocus();
    fireEvent.keyDown(document, { key: 'ArrowUp' });
    expect(screen.getByRole('menuitem', { name: 'Sign out' })).toHaveFocus();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Settings' }));
    expect(settings).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  });

  it('leaves out Settings and sign out when they do not apply, and closes on Escape', () => {
    render(<HeaderMenu onShortcuts={vi.fn()} onServerInfo={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Menu' }));
    expect(screen.getAllByRole('menuitem').map((item) => item.textContent?.trim())).toEqual([
      'Keyboard shortcuts', 'Server information',
    ]);
    expect(screen.queryByText(/Signed in as/)).not.toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Menu' })).toHaveFocus();
  });
});
