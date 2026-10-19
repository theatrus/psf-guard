import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';

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
 * It belongs beside a `.view-toolbar.sticky` with the same parent.
 */
export default function ToolbarDock({ children }: { children: ReactNode }) {
  const sentinelRef = useRef<HTMLDivElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const [toolbarHeight, setToolbarHeight] = useState(0);
  const [docked, setDocked] = useState(false);

  useLayoutEffect(() => {
    const dock = dockRef.current;
    const toolbar = dock?.parentElement?.querySelector<HTMLElement>(':scope > .view-toolbar.sticky');
    if (!dock || !toolbar) return;
    const measure = () => {
      const height = toolbar.getBoundingClientRect().height;
      dock.style.setProperty('--toolbar-dock-top', `${height}px`);
      setToolbarHeight(Math.round(height));
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(toolbar);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const sentinel = sentinelRef.current;
    const dock = dockRef.current;
    if (!sentinel || !dock || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(([entry]) => {
      const top = entry.rootBounds?.top ?? toolbarHeight;
      // A short window lets the block scroll away (see App.css); it is
      // docked only while it sticks.
      setDocked(!entry.isIntersecting
        && entry.boundingClientRect.top < top
        && getComputedStyle(dock).position === 'sticky');
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
      >
        {children}
      </div>
    </>
  );
}
