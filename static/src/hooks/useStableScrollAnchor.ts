import { useLayoutEffect, type RefObject } from 'react';

interface AnchorSnapshot {
  id: string;
  offsetFromStickyTop: number;
}

/**
 * Keeps the first visible grid item in place when previews, stack status, or
 * a background image refresh changes the height of content above it.
 */
export function useStableScrollAnchor(
  contentRef: RefObject<HTMLElement | null>,
  enabled: boolean,
) {
  useLayoutEffect(() => {
    const content = contentRef.current;
    const scroller = content?.closest<HTMLElement>('.app-main');
    if (!enabled || !content || !scroller || typeof ResizeObserver === 'undefined') return;

    let snapshot: AnchorSnapshot | null = null;
    let captureFrame = 0;
    let adjusting = false;

    const stickyTop = () => {
      const scrollerTop = scroller.getBoundingClientRect().top;
      const controls = content.querySelector<HTMLElement>('.image-controls.sticky');
      return Math.max(scrollerTop, controls?.getBoundingClientRect().bottom ?? scrollerTop);
    };

    const findAnchor = (id?: string) => {
      const candidates = content.querySelectorAll<HTMLElement>('[data-scroll-anchor]');
      if (id) {
        for (const candidate of candidates) {
          if (candidate.dataset.scrollAnchor === id) return candidate;
        }
        return null;
      }

      const cutoff = stickyTop();
      const viewportBottom = scroller.getBoundingClientRect().bottom;
      for (const candidate of candidates) {
        const rect = candidate.getBoundingClientRect();
        if (rect.bottom > cutoff + 1 && rect.top < viewportBottom) return candidate;
      }
      return null;
    };

    const capture = () => {
      if (scroller.scrollTop <= 1) {
        snapshot = null;
        return;
      }
      const anchor = findAnchor();
      snapshot = anchor
        ? {
            id: anchor.dataset.scrollAnchor!,
            offsetFromStickyTop: anchor.getBoundingClientRect().top - stickyTop(),
          }
        : null;
    };

    const scheduleCapture = () => {
      if (adjusting || captureFrame) return;
      captureFrame = requestAnimationFrame(() => {
        captureFrame = 0;
        capture();
      });
    };

    const preserve = () => {
      if (!snapshot || scroller.scrollTop <= 1) {
        capture();
        return;
      }

      const anchor = findAnchor(snapshot.id);
      if (!anchor) {
        capture();
        return;
      }

      const currentOffset = anchor.getBoundingClientRect().top - stickyTop();
      const delta = currentOffset - snapshot.offsetFromStickyTop;
      if (Math.abs(delta) < 0.5) return;

      adjusting = true;
      scroller.scrollTop += delta;
      requestAnimationFrame(() => {
        adjusting = false;
        capture();
      });
    };

    capture();
    scroller.addEventListener('scroll', scheduleCapture, { passive: true });
    const observer = new ResizeObserver(preserve);
    observer.observe(content);
    observer.observe(scroller);

    return () => {
      scroller.removeEventListener('scroll', scheduleCapture);
      observer.disconnect();
      if (captureFrame) cancelAnimationFrame(captureFrame);
    };
  }, [contentRef, enabled]);
}
