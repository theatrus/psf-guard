import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { nextTooTall, pageRoom } from '../utils/toolbarDock';

/**
 * Keeps its children in sight under the view's sticky toolbar. They scroll
 * with the page until they reach the toolbar, stay there, docked, while the
 * page scrolls on under them, and leave again when it scrolls back.
 *
 * `position: sticky` does the docking, so docking moves nothing else on the
 * page: Safari and the macOS app have no scroll anchoring to put it back.
 * This sets how far down the block stops, the toolbar's height as that
 * changes, and marks the block `data-docked` while it is docked. A one-pixel
 * sentinel just above it tells when: once the sentinel passes under the
 * toolbar, the block has stuck.
 *
 * The toolbar and the block leave the page about half the window. A block
 * too tall for that, such as on a phone on its side, is marked
 * `data-too-tall` and scrolls with the page (see DOCK_SHARE). Its height is
 * measured with the header in sight, so a phone's header sliding away as
 * the page scrolls down does not change the answer. A list that opens from
 * the block hangs below it rather than growing it (App.css).
 *
 * `onCoverChange` hears when the block changes size while the page is
 * scrolled past its place, or docks again: either moves what it covers.
 *
 * It belongs beside a `.view-toolbar.sticky` with the same parent.
 */
export default function ToolbarDock({ children, onCoverChange }: {
  children: ReactNode;
  onCoverChange?: () => void;
}) {
  const sentinelRef = useRef<HTMLDivElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const [toolbarHeight, setToolbarHeight] = useState(0);
  const [passed, setPassed] = useState(false);
  const [tooTall, setTooTall] = useState(false);
  const docked = passed && !tooTall;
  // For the observers, which outlive a render.
  const passedRef = useRef(passed);
  passedRef.current = passed;
  const tooTallRef = useRef(tooTall);
  const onCoverChangeRef = useRef(onCoverChange);
  onCoverChangeRef.current = onCoverChange;

  useLayoutEffect(() => {
    const dock = dockRef.current;
    const toolbar = dock?.parentElement?.querySelector<HTMLElement>(':scope > .view-toolbar.sticky');
    if (!dock || !toolbar) return;
    const scroller = dock.closest<HTMLElement>('.app-main');
    let dockHeight: number | null = null;
    const measure = () => {
      const height = toolbar.getBoundingClientRect().height;
      dock.style.setProperty('--toolbar-dock-top', `${height}px`);
      setToolbarHeight(Math.round(height));
      const room = pageRoom(scroller);
      if (room <= 0) return;
      const next = nextTooTall(tooTallRef.current, (height + dock.offsetHeight) / room);
      if (next !== tooTallRef.current) {
        tooTallRef.current = next;
        setTooTall(next);
      }
      const resized = dockHeight !== null && dockHeight !== dock.offsetHeight;
      dockHeight = dock.offsetHeight;
      if (resized && passedRef.current) onCoverChangeRef.current?.();
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(toolbar);
    observer.observe(dock);
    if (scroller) observer.observe(scroller);
    return () => observer.disconnect();
  }, []);

  // Docking again once it fits, it covers the top of the page again.
  // Reaching the toolbar by scrolling covers nothing that was in sight.
  const wasTooTall = useRef(tooTall);
  useEffect(() => {
    if (wasTooTall.current && !tooTall && passedRef.current) onCoverChangeRef.current?.();
    wasTooTall.current = tooTall;
  }, [tooTall]);

  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(([entry]) => {
      const top = entry.rootBounds?.top ?? toolbarHeight;
      setPassed(!entry.isIntersecting && entry.boundingClientRect.top < top);
    }, {
      root: sentinel.closest('.app-main'),
      rootMargin: `-${toolbarHeight}px 0px 0px 0px`,
    });
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [toolbarHeight]);

  return (
    <>
      <div ref={sentinelRef} className="toolbar-dock-sentinel" aria-hidden="true" />
      <div
        ref={dockRef}
        className="toolbar-dock"
        data-docked={docked || undefined}
        data-too-tall={tooTall || undefined}
      >
        {children}
      </div>
    </>
  );
}
