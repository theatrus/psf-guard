import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { CSSProperties, MouseEvent as ReactMouseEvent } from 'react';
import { createPortal } from 'react-dom';

interface QualityReasonPopoverProps {
  reason?: string | null;
  details?: string | null;
}

export default function QualityReasonPopover({ reason, details }: QualityReasonPopoverProps) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<CSSProperties>({});
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const popoverId = useId();

  const updatePosition = useCallback(() => {
    const rect = buttonRef.current?.getBoundingClientRect();
    if (!rect) return;
    const gutter = 12;
    const width = Math.min(360, window.innerWidth - gutter * 2);
    const left = Math.max(gutter, Math.min(rect.left, window.innerWidth - width - gutter));
    const spaceBelow = window.innerHeight - rect.bottom;
    const spaceAbove = rect.top;
    setPosition(spaceBelow >= 180 || spaceBelow >= spaceAbove
      ? { top: rect.bottom + 8, left, width }
      : { bottom: window.innerHeight - rect.top + 8, left, width });
  }, []);

  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: PointerEvent) => {
      const target = event.target as Node;
      if (buttonRef.current?.contains(target) || popoverRef.current?.contains(target)) return;
      setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      buttonRef.current?.focus();
    };
    const reposition = () => updatePosition();
    document.addEventListener('pointerdown', closeOnOutsideClick);
    document.addEventListener('keydown', closeOnEscape, true);
    window.addEventListener('resize', reposition);
    window.addEventListener('scroll', reposition, true);
    return () => {
      document.removeEventListener('pointerdown', closeOnOutsideClick);
      document.removeEventListener('keydown', closeOnEscape, true);
      window.removeEventListener('resize', reposition);
      window.removeEventListener('scroll', reposition, true);
    };
  }, [open, updatePosition]);

  const toggle = (event: ReactMouseEvent) => {
    event.stopPropagation();
    if (!open) updatePosition();
    setOpen(value => !value);
  };

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        className="sequence-reason-trigger"
        aria-expanded={open}
        aria-controls={popoverId}
        aria-haspopup="dialog"
        aria-label={`${open ? 'Hide' : 'Show'} quality reason`}
        onClick={toggle}
        onDoubleClick={event => event.stopPropagation()}
      >
        Reason
      </button>
      {open && createPortal(
        <div
          ref={popoverRef}
          id={popoverId}
          className="sequence-reason-popover"
          role="dialog"
          aria-label="Quality reason"
          style={position}
          onClick={event => event.stopPropagation()}
          onDoubleClick={event => event.stopPropagation()}
        >
          <div className="sequence-reason-popover-header">
            <strong>{reason ? 'Review reason' : 'Score reason'}</strong>
            <button
              type="button"
              aria-label="Close quality reason"
              onClick={() => {
                setOpen(false);
                buttonRef.current?.focus();
              }}
            >
              ×
            </button>
          </div>
          {reason && <p>{reason}</p>}
          {details && details !== reason && (
            <>
              {reason && <span className="sequence-reason-evidence-label">Evidence</span>}
              <p>{details}</p>
            </>
          )}
        </div>,
        document.body,
      )}
    </>
  );
}
