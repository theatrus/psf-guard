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
 * None of this waits for the browser to draw. The browser reports a scroll,
 * and runs resize observers, only when it next draws, and a busy WebKit can
 * go seconds without drawing while keys and data keep arriving. A scroll
 * learned only then would be read against the next image's panels. So a
 * key or a press first reads where the reader is, before the app acts on
 * it, and a change inside the panels is made good as it lands.
 *
 * `scroller` is the sidebar itself; its children are the panels.
 */
export function useSidebarScrollAnchor(scroller: HTMLElement | null) {
  useLayoutEffect(() => {
    if (!scroller || typeof ResizeObserver === 'undefined') return;

    let anchor: Anchor | null = null;
    // Where the sidebar was and how tall its content was when the anchor
    // was last chosen or kept. A move from there with the content the same
    // height is the reader's; with the height changed it is a panel's, such
    // as the scroll the browser clamps when the content gets shorter.
    let knownTop = scroller.scrollTop;
    let knownHeight = scroller.scrollHeight;

    const scrollerTop = () => scroller.getBoundingClientRect().top;

    const capture = () => {
      anchor = null;
      knownTop = scroller.scrollTop;
      knownHeight = scroller.scrollHeight;
      if (knownTop <= 1) return;
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
      knownTop = scroller.scrollTop;
      knownHeight = scroller.scrollHeight;
    };

    // Whether the sidebar moved since the hook last looked, and who moved it.
    const sync = () => {
      // The content changed height: a panel moved the scroll, not the reader.
      if (scroller.scrollHeight !== knownHeight) {
        preserve();
        return;
      }
      // Where the last change left it, or where the reader already was.
      if (Math.abs(scroller.scrollTop - knownTop) < 1) return;
      capture();
    };

    // Read on every scroll, not on the next frame: a panel that changes
    // size in the same frame as the reader scrolls must not undo the scroll.
    scroller.addEventListener('scroll', sync, { passive: true });
    // Before the app acts on a key or a press, such as an arrow key moving
    // to the next image, so a scroll not yet reported is still the reader's.
    window.addEventListener('keydown', sync, { capture: true });
    window.addEventListener('pointerdown', sync, { capture: true });

    const resizes = new ResizeObserver(preserve);
    for (const child of Array.from(scroller.children)) resizes.observe(child);
    // A panel that appears or goes, such as the copy switch, or that changes
    // inside, such as the next light's data arriving: checked as the change
    // lands rather than when the browser next draws.
    const mutations = typeof MutationObserver === 'undefined'
      ? null
      : new MutationObserver((records) => {
          for (const record of records) {
            if (record.target !== scroller) continue;
            record.addedNodes.forEach((node) => {
              if (node instanceof Element) resizes.observe(node);
            });
            record.removedNodes.forEach((node) => {
              if (node instanceof Element) resizes.unobserve(node);
            });
          }
          sync();
        });
    mutations?.observe(scroller, { childList: true, characterData: true, subtree: true });
    capture();

    return () => {
      scroller.removeEventListener('scroll', sync);
      window.removeEventListener('keydown', sync, { capture: true });
      window.removeEventListener('pointerdown', sync, { capture: true });
      resizes.disconnect();
      mutations?.disconnect();
    };
  }, [scroller]);
}
