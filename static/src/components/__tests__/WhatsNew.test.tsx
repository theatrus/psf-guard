import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useHotkeys } from 'react-hotkeys-hook';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import WhatsNew from '../WhatsNew';
import HeaderMenu from '../header/HeaderMenu';
import { acknowledgeVersion, WHATS_NEW_KEY } from '../../updates/whatsNew';

vi.mock('../../updates/highlights.json', () => ({ default: [
  { version: '1.2.0', deprecations: [], upgradeNotes: [{ title: 'Move settings', body: 'Copy the settings folder.' }], highlights: [{ title: 'New stacks', body: 'Build stacks.' }] },
  { version: '1.1.0', deprecations: [{ title: 'Old export removed', body: 'Use the new export.' }], upgradeNotes: [], highlights: [] },
] }));

const props = { version: '1.2.0', requested: false, onClose: vi.fn() };
const dialog = () => screen.queryByRole('dialog');
const suppress = () => {
  fireEvent.click(screen.getByRole('checkbox', { name: "Don't show again for this version" }));
  fireEvent.click(screen.getByRole('button', { name: 'Done' }));
};

beforeEach(() => { localStorage.removeItem(WHATS_NEW_KEY); });
afterEach(() => { vi.restoreAllMocks(); localStorage.removeItem(WHATS_NEW_KEY); });

describe("What's new", () => {
  it('shows the current release on first visit, with upgrade notes before features', () => {
    render(<WhatsNew {...props} />);
    expect(screen.getByRole('dialog', { name: "What's new in PSF Guard v1.2.0" })).toBeVisible();
    expect(screen.getAllByRole('heading', { level: 3 }).map(h => h.textContent)).toEqual(['Upgrade notes', 'New features']);
    expect(screen.queryByText('Old export removed')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Full release notes for v1.2.0' })).toHaveAttribute('href', 'https://github.com/theatrus/psf-guard/releases/tag/v1.2.0');
  });

  it.each(['Done', 'Close dialog', 'Escape', 'backdrop'])('dismisses with %s without saving', (method) => {
    const { unmount } = render(<WhatsNew {...props} />);
    if (method === 'Escape') fireEvent.keyDown(document, { key: 'Escape' });
    else if (method === 'backdrop') fireEvent.mouseDown(screen.getByRole('presentation'));
    else fireEvent.click(screen.getByRole('button', { name: method }));
    expect(dialog()).not.toBeInTheDocument();
    expect(localStorage.getItem(WHATS_NEW_KEY)).toBeNull();
    unmount();
    render(<WhatsNew {...props} />);
    expect(dialog()).toBeVisible();
  });

  it('remembers this version, allows menu reopening and announces a later release', () => {
    const { rerender } = render(<WhatsNew key="old" {...props} version="1.1.0" />);
    suppress();
    rerender(<WhatsNew key="reload" {...props} version="1.1.0" />);
    expect(dialog()).not.toBeInTheDocument();
    rerender(<WhatsNew key="reload" {...props} version="1.1.0" requested />);
    expect(dialog()).toBeVisible();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
    rerender(<WhatsNew key="upgrade" {...props} />);
    expect(dialog()).toBeVisible();
    expect(screen.queryByText('Old export removed')).not.toBeInTheDocument();
  });

  it('puts skipped deprecations before all upgrade notes and features', () => {
    localStorage.setItem(WHATS_NEW_KEY, '1.0.0');
    render(<WhatsNew {...props} />);
    expect(screen.getAllByRole('heading', { level: 3 }).map(h => h.textContent)).toEqual(['Deprecations', 'Upgrade notes', 'New features']);
    expect(screen.getByText('Use the new export.')).toBeInTheDocument();
  });

  it('does not automatically open for downgrades or versions absent from the bundle', () => {
    localStorage.setItem(WHATS_NEW_KEY, '1.2.0');
    const { rerender } = render(<WhatsNew key="downgrade" {...props} version="1.1.0" />);
    expect(dialog()).not.toBeInTheDocument();
    rerender(<WhatsNew key="unknown" {...props} version="2.0.0" />);
    expect(dialog()).not.toBeInTheDocument();
    rerender(<WhatsNew key="unknown" {...props} version="2.0.0" requested />);
    expect(screen.getByText('No release highlights are available for this version.')).toBeInTheDocument();
  });

  it('handles loading, invalid versions, bad saved values and build metadata', () => {
    const { rerender } = render(<WhatsNew key="loading" {...props} version={undefined} />);
    expect(dialog()).not.toBeInTheDocument();
    rerender(<WhatsNew key="invalid" {...props} version="test" />);
    expect(dialog()).not.toBeInTheDocument();
    localStorage.setItem(WHATS_NEW_KEY, 'garbage');
    rerender(<WhatsNew key="valid" {...props} version="v1.2.0+build7" />);
    expect(dialog()).toBeVisible();
  });

  it('waits until another app dialog closes', () => {
    const { rerender } = render(<WhatsNew {...props} blocked />);
    expect(dialog()).not.toBeInTheDocument();
    rerender(<WhatsNew {...props} />);
    expect(dialog()).toBeVisible();
  });

  it('closes even when browser storage is unavailable', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked'); });
    render(<WhatsNew {...props} />);
    suppress();
    expect(dialog()).not.toBeInTheDocument();
  });

  it('does not overwrite a newer acknowledgment from another tab', () => {
    localStorage.setItem(WHATS_NEW_KEY, '2.0.0');
    acknowledgeVersion('1.2.0');
    expect(localStorage.getItem(WHATS_NEW_KEY)).toBe('2.0.0');
  });

  it('blocks grading, navigation and Escape shortcuts behind the popup', async () => {
    const backgroundShortcut = vi.fn();
    function BackgroundView() {
      useHotkeys('a,right,escape', backgroundShortcut, []);
      return <WhatsNew {...props} />;
    }
    const user = userEvent.setup();
    render(<BackgroundView />);
    await user.keyboard('a{ArrowRight}');
    expect(backgroundShortcut).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    expect(dialog()).not.toBeInTheDocument();
    expect(backgroundShortcut).not.toHaveBeenCalled();
  });

  it('opens from the menu and closes the menu', () => {
    const onWhatsNew = vi.fn();
    render(<HeaderMenu onWhatsNew={onWhatsNew} onServerInfo={vi.fn()} onShortcuts={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Menu' }));
    fireEvent.click(within(screen.getByRole('menu')).getByRole('menuitem', { name: "What's new" }));
    expect(onWhatsNew).toHaveBeenCalledOnce();
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Menu' })).toHaveFocus();
  });
});
