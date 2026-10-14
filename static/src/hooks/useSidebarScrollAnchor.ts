import { useLayoutEffect } from 'react';

interface Anchor {
  element: Element;
  /** Its top, from the top of the sidebar. */
  offset: number;
}

/**
 * Keeps the reader's place in a scrolling sidebar while its panels open,
 * fold or change size: the next image's Calibration panel opens because
 * that light has a finding, or a panel shrinks while it loads the next
 * image's data and grows back when it arrives. The anchor is the first panel
 * that starts in view, else the one the top of the view is in, and its
 * distance from the top of the sidebar is kept. A sidebar at the top stays
 * at the top.
 *
 * The anchor is chosen when the reader scrolls and kept until they scroll
 * again, through any number of changes. Choosing again after each change
 * loses the place: a panel above that shrinks while loading brings another
 * panel into view, which would then become the anchor and stay put while
 * the first panel grows back. WebKit has no `overflow-anchor`, so this is
 * all the anchoring Safari and the macOS app get.
 *
 * `scroller` is the sidebar itself; its children are the panels.
 */
export function useSidebarScrollAnchor(scroller: HTMLElement | null) {
  useLayoutEffect(() => {
    if (!scroller || typeof ResizeObserver === 'undefined') return;

    let anchor: Anchor | null = null;
    // Where the last change left the sidebar and how tall its content was,
    // so the scroll events a change fires are not taken for the reader's.
    // The browser fires one when it clamps the scroll of content that got
    // shorter, sometimes before the resize reaches `preserve`.
    let settledTop: number | null = null;
    let settledHeight = scroller.scrollHeight;

    const scrollerTop = () => scroller.getBoundingClientRect().top;

    const capture = () => {
      anchor = null;
      settledTop = null;
      settledHeight = scroller.scrollHeight;
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
      if (!anchor?.element.isConnected) {
        capture();
        return;
      }
      const delta = anchor.element.getBoundingClientRect().top - scrollerTop() - anchor.offset;
      // Clamped at either end, the sidebar may not get all the way there; the
      // anchor stays, and the next change that makes room finishes the job.
      if (Math.abs(delta) >= 0.5) scroller.scrollTop += delta;
      settledTop = scroller.scrollTop;
      settledHeight = scroller.scrollHeight;
    };

    // Read on every scroll, not on the next frame: a panel that changes
    // size in the same frame as the reader scrolls must not undo the scroll.
    const onScroll = () => {
      // The content changed height: a panel moved the scroll, not the reader.
      if (scroller.scrollHeight !== settledHeight) {
        preserve();
        return;
      }
      // The scroll an adjustment or a clamp already accounted for.
      if (settledTop !== null && Math.abs(scroller.scrollTop - settledTop) < 1) return;
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
    scroller.addEventListener('scroll', onScroll, { passive: true });

    return () => {
      scroller.removeEventListener('scroll', onScroll);
      resizes.disconnect();
      mutations?.disconnect();
    };
  }, [scroller]);
}
