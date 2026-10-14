import { useLayoutEffect } from 'react';

interface Anchor {
  element: Element;
  /** Its top, from the top of the sidebar. */
  offset: number;
}

/**
 * Keeps the reader's place in a scrolling sidebar while its panels open,
 * fold or change size: the next image's Calibration panel opens because
 * that light has a finding, or a panel's data arrives. The anchor is the
 * first panel that starts in view, else the one the top of the view is in,
 * and its distance from the top of the sidebar is kept. A sidebar at the
 * top stays at the top.
 *
 * `scroller` is the sidebar itself; its children are the panels.
 */
export function useSidebarScrollAnchor(scroller: HTMLElement | null) {
  useLayoutEffect(() => {
    if (!scroller || typeof ResizeObserver === 'undefined') return;

    let anchor: Anchor | null = null;

    const scrollerTop = () => scroller.getBoundingClientRect().top;

    // Read on every scroll, not on the next frame: a panel that changes
    // size in the same frame as the reader scrolls must not undo the scroll.
    const capture = () => {
      anchor = null;
      if (scroller.scrollTop <= 1) return;
      const { top, bottom } = scroller.getBoundingClientRect();
      for (const element of Array.from(scroller.children)) {
        const rect = element.getBoundingClientRect();
        if (rect.height === 0) continue;
        if (rect.top >= top - 1 && rect.top < bottom) {
          anchor = { element, offset: rect.top - top };
          return;
        }
        // A panel cut by the top of the view: kept unless a later one starts in view.
        if (rect.top < top && rect.bottom > top) anchor = { element, offset: rect.top - top };
      }
    };

    const preserve = () => {
      if (anchor?.element.isConnected) {
        const delta = anchor.element.getBoundingClientRect().top - scrollerTop() - anchor.offset;
        if (Math.abs(delta) >= 0.5) scroller.scrollTop += delta;
      }
      capture();
    };

    const resizes = new ResizeObserver(preserve);
    const observeChildren = () => {
      for (const child of Array.from(scroller.children)) resizes.observe(child);
    };
    // A panel that appears or goes, such as the copy switch.
    const mutations = typeof MutationObserver === 'undefined'
      ? null
      : new MutationObserver(() => {
          observeChildren();
          preserve();
        });

    observeChildren();
    mutations?.observe(scroller, { childList: true });
    capture();
    scroller.addEventListener('scroll', capture, { passive: true });

    return () => {
      scroller.removeEventListener('scroll', capture);
      resizes.disconnect();
      mutations?.disconnect();
    };
  }, [scroller]);
}
