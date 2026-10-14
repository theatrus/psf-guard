import { useId, type ReactNode } from 'react';
import { usePopover } from './usePopover';
import './toolbar.css';

interface ToolbarMenuButtonProps {
  /** The button's name, shown beside its icon on a wide screen. */
  label: string;
  icon: ReactNode;
  /** Only the icon shows; the name is the tooltip and what a screen
   *  reader says. */
  iconOnly?: boolean;
  /** Close once a button in the panel is used, as a menu does. */
  closeOnAction?: boolean;
  /** A setting in the panel is off its default, such as "newest first":
   *  the button is marked set and says it in its name. */
  note?: string;
  panelClassName?: string;
  children: ReactNode;
}

/**
 * A toolbar button that opens a panel: View, for how the view draws, and
 * ⋯, for the tools used too rarely to keep in sight. The panel stays
 * mounted while closed so the settings in it keep their drafts.
 */
export default function ToolbarMenuButton({
  label,
  icon,
  iconOnly = false,
  closeOnAction = false,
  note,
  panelClassName,
  children,
}: ToolbarMenuButtonProps) {
  const panelId = useId();
  const { open, setOpen, rootRef, buttonRef } = usePopover();
  const name = note ? `${label} (${note})` : label;
  return (
    <div className="toolbar-popover" ref={rootRef}>
      <button
        ref={buttonRef}
        type="button"
        className={`tool-button toolbar-menu-button${note ? ' is-set' : ''}`}
        aria-haspopup="true"
        aria-expanded={open}
        aria-controls={panelId}
        aria-label={iconOnly || note ? name : undefined}
        title={note ? `${label}: ${note}` : label}
        onClick={() => setOpen((shown) => !shown)}
      >
        {icon}
        {!iconOnly && <span className="tool-button-label">{label}</span>}
      </button>
      <div
        id={panelId}
        className={`toolbar-menu${panelClassName ? ` ${panelClassName}` : ''}`}
        role="group"
        aria-label={label}
        hidden={!open}
        onClick={closeOnAction ? (event) => {
          if ((event.target as Element).closest('button')) setOpen(false);
        } : undefined}
      >
        {children}
      </div>
    </div>
  );
}
