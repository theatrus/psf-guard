import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';

/** The most of the scrolling region the toolbar and the docked block may
 *  cover together; the page under them keeps the rest. */
export const DOCK_MAX_SHARE = 0.55;

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
 * A block that would cover more than DOCK_MAX_SHARE of the window with the
 * toolbar, such as one holding a long list on a laptop or a phone on its
 * side, does not dock: it is marked `data-too-tall` and scrolls with the
 * page. When it grows too tall while docked, the page scrolls to keep it
 * where it was on screen, so what grew it, such as a list opening, stays in
 * sight. `onDockedResize` hears when it changes size while docked, which
 * moves the page under it.
 *
 * It belongs beside a `.view-toolbar.sticky` with the same parent.
 */
export default function ToolbarDock({ children, onDockedResize }: {
  children: ReactNode;
  onDockedResize?: () => void;
}) {
  const sentinelRef = useRef<HTMLDivElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const [toolbarHeight, setToolbarHeight] = useState(0);
  const [passed, setPassed] = useState(false);
  const [tooTall, setTooTall] = useState(false);
  const docked = passed && !tooTall;
  // For the observers, which outlive a render.
  const dockedRef = useRef(docked);
  dockedRef.current = docked;
  const tooTallRef = useRef(tooTall);
  const onResizeRef = useRef(onDockedResize);
  onResizeRef.current = onDockedResize;
  // Where the block was on screen as it grew too tall to dock.
  const placeRef = useRef<number | null>(null);

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
      const room = scroller?.clientHeight ?? window.innerHeight;
      const nextTooTall = room > 0 && height + dock.offsetHeight > room * DOCK_MAX_SHARE;
      if (nextTooTall !== tooTallRef.current) {
        tooTallRef.current = nextTooTall;
        placeRef.current = nextTooTall && dockedRef.current ? dock.getBoundingClientRect().top : null;
        setTooTall(nextTooTall);
      }
      const resized = dockHeight !== null && dockHeight !== dock.offsetHeight;
      dockHeight = dock.offsetHeight;
      if (resized && !nextTooTall && dockedRef.current) onResizeRef.current?.();
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(toolbar);
    observer.observe(dock);
    if (scroller) observer.observe(scroller);
    return () => observer.disconnect();
  }, []);

  // Grown too tall while docked: it now sits where the page puts it, so
  // scroll the page to bring it back to where it was.
  useLayoutEffect(() => {
    const place = placeRef.current;
    placeRef.current = null;
    const dock = dockRef.current;
    const scroller = dock?.closest<HTMLElement>('.app-main');
    if (place === null || !dock || !scroller) return;
    scroller.scrollTop += dock.getBoundingClientRect().top - place;
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
