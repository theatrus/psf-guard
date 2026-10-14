import { useCallback, useLayoutEffect, useRef } from 'react';

/** The scrolling region a view's cards live in. */
const SCROLLER = '.app-main';

/**
 * Keeps the current image where the eye left it when the frames change
 * order. Call `capture` just before changing the order: once the frames are
 * redrawn, the current card goes back to the same height on screen. When
 * the order changed some other way, such as Back, or the card was off
 * screen, the card is brought into view instead.
 */
export function useReorderAnchor(order: string, findCurrent: () => HTMLElement | null) {
  const offsetRef = useRef<number | null>(null);
  const orderRef = useRef(order);
  const findRef = useRef(findCurrent);
  findRef.current = findCurrent;

  const capture = useCallback(() => {
    const card = findRef.current();
    const scroller = card?.closest<HTMLElement>(SCROLLER);
    if (!card || !scroller) {
      offsetRef.current = null;
      return;
    }
    const cardRect = card.getBoundingClientRect();
    const view = scroller.getBoundingClientRect();
    const onScreen = cardRect.bottom > view.top && cardRect.top < view.bottom;
    offsetRef.current = onScreen ? cardRect.top - view.top : null;
  }, []);

  useLayoutEffect(() => {
    if (orderRef.current === order) return;
    orderRef.current = order;
    const offset = offsetRef.current;
    offsetRef.current = null;

    const place = () => {
      const card = findRef.current();
      const scroller = card?.closest<HTMLElement>(SCROLLER);
      if (!card || !scroller) return;
      const cardRect = card.getBoundingClientRect();
      const view = scroller.getBoundingClientRect();
      if (offset !== null) {
        const delta = cardRect.top - view.top - offset;
        if (Math.abs(delta) >= 0.5) scroller.scrollTop += delta;
      } else if (cardRect.bottom <= view.top || cardRect.top >= view.bottom) {
        // jsdom has no layout; guard like the other scroll helpers.
        card.scrollIntoView?.({ block: 'center' });
      }
    };

    place();
    // Previews and lazy cards settle a frame later.
    const frame = requestAnimationFrame(place);
    return () => cancelAnimationFrame(frame);
  }, [order]);

  return capture;
}
