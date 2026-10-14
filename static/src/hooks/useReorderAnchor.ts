import { useCallback, useLayoutEffect, useRef } from 'react';

/** The scrolling region a view's cards live in. */
const SCROLLER = '.app-main';

/** How a flip of the order treats the page. */
export type ReorderMode =
  /** The person has moved: the current card stays at the same height on
   *  screen while the frames turn around it. */
  | 'anchor'
  /** The view is as it opened: the page stays where it is and the cursor
   *  moves to the new first card, which takes the old one's place. */
  | 'rest';

/**
 * Whether a view is as it opened: the cursor is still on the first card,
 * which the view chose, nobody has moved it by keys or a click, and that
 * card has not been scrolled out of sight.
 */
export function viewIsAtRest({
  currentId,
  firstId,
  touched,
  firstOnScreen,
}: {
  currentId: number | null;
  firstId: number | null;
  touched: boolean;
  firstOnScreen: boolean;
}): boolean {
  return !touched && currentId !== null && currentId === firstId && firstOnScreen;
}

/** Whether a card shows in its scrolling region. Without a layout to
 *  measure, as in tests, it counts as shown. */
export function cardIsOnScreen(card: HTMLElement | null | undefined): boolean {
  const scroller = card?.closest<HTMLElement>(SCROLLER);
  if (!card || !scroller) return true;
  const cardRect = card.getBoundingClientRect();
  const view = scroller.getBoundingClientRect();
  return cardRect.bottom > view.top && cardRect.top < view.bottom;
}

/**
 * Keeps the person's place when the frames change order. Call `capture`
 * just before changing the order:
 *
 * - `anchor`: once the frames are redrawn, the current card goes back to
 *   the same height on screen. If it was off screen, or the order changed
 *   some other way, such as Back, it is brought into view instead.
 * - `rest`: the page stays where it is. The caller moves the cursor to the
 *   new first card in the same change, so it lands where the old one was.
 */
export function useReorderAnchor(order: string, findCurrent: () => HTMLElement | null) {
  const pendingRef = useRef<{ mode: 'anchor'; offset: number | null } | { mode: 'rest' } | null>(null);
  const orderRef = useRef(order);
  const findRef = useRef(findCurrent);
  findRef.current = findCurrent;

  const capture = useCallback((mode: ReorderMode = 'anchor') => {
    if (mode === 'rest') {
      pendingRef.current = { mode: 'rest' };
      return;
    }
    const card = findRef.current();
    const scroller = card?.closest<HTMLElement>(SCROLLER);
    if (!card || !scroller) {
      pendingRef.current = { mode: 'anchor', offset: null };
      return;
    }
    const cardRect = card.getBoundingClientRect();
    const view = scroller.getBoundingClientRect();
    const onScreen = cardRect.bottom > view.top && cardRect.top < view.bottom;
    pendingRef.current = { mode: 'anchor', offset: onScreen ? cardRect.top - view.top : null };
  }, []);

  useLayoutEffect(() => {
    if (orderRef.current === order) return;
    orderRef.current = order;
    const pending = pendingRef.current ?? { mode: 'anchor', offset: null };
    pendingRef.current = null;
    // At rest, nothing moves: the new first card takes the old one's place.
    if (pending.mode === 'rest') return;
    const offset = pending.offset;

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
