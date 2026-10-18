import { useEffect, useRef, useState } from 'react';
import { Info, Keyboard, LogOut, Menu, Settings, Sparkles } from 'lucide-react';

interface HeaderMenuProps {
  /** Present when the person may open Settings. */
  onSettings?: () => void;
  onWhatsNew: () => void;
  onShortcuts: () => void;
  onServerInfo: () => void;
  /** Who is signed in, when the server asks people to sign in. */
  signedInAs?: string | null;
  /** Present when the server asks people to sign in. */
  onSignOut?: () => void;
}

/**
 * App settings, release notes, help and account actions share one menu.
 */
export default function HeaderMenu({ onSettings, onWhatsNew, onShortcuts, onServerInfo, signedInAs, onSignOut }: HeaderMenuProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    listRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onPointer = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
        buttonRef.current?.focus();
        return;
      }
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      const items = Array.from(listRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
      if (items.length === 0) return;
      event.preventDefault();
      const at = items.indexOf(document.activeElement as HTMLElement);
      const step = event.key === 'ArrowDown' ? 1 : -1;
      items[(at + step + items.length) % items.length].focus();
    };
    document.addEventListener('pointerdown', onPointer);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('pointerdown', onPointer);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const choose = (action: () => void) => () => {
    setOpen(false);
    buttonRef.current?.focus();
    action();
  };

  return (
    <div className="header-menu" ref={rootRef}>
      <button
        ref={buttonRef}
        type="button"
        className="header-button utility-button header-menu-button"
        aria-label="Menu"
        title="Settings, help and server"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <Menu size={16} aria-hidden="true" />
      </button>
      {open && (
        <div className="header-menu-list" role="menu" aria-label="Menu" ref={listRef}>
          {signedInAs && <div className="header-menu-who">Signed in as {signedInAs}</div>}
          {onSettings && (
            <button type="button" role="menuitem" onClick={choose(onSettings)}>
              <Settings size={15} aria-hidden="true" /> Settings
            </button>
          )}
          <button type="button" role="menuitem" onClick={choose(onWhatsNew)}>
            <Sparkles size={15} aria-hidden="true" /> What's new
          </button>
          <button type="button" role="menuitem" onClick={choose(onShortcuts)}>
            <Keyboard size={15} aria-hidden="true" /> Keyboard shortcuts
          </button>
          <button type="button" role="menuitem" onClick={choose(onServerInfo)}>
            <Info size={15} aria-hidden="true" /> Server information
          </button>
          {onSignOut && (
            <button type="button" role="menuitem" onClick={choose(onSignOut)}>
              <LogOut size={15} aria-hidden="true" /> Sign out
            </button>
          )}
        </div>
      )}
    </div>
  );
}
