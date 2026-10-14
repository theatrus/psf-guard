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

/** How long after a flip the current card is held at its height on
 *  screen, while previews and lazy cards draw at their real sizes. */
export const SETTLE_MS = 800;

/** Keys that close a panel or move focus, and do not move the page. */
const QUIET_KEYS = new Set(['Escape', 'Tab', 'Shift', 'Control', 'Alt', 'Meta']);

/**
 * Keep `place` holding the current card for SETTLE_MS: every frame, and on
 * every resize of the scrolling region's content. useStableScrollAnchor
 * keeps the first visible card in place when content above it resizes; its
 * observer was made first, so this one runs after it and has the last word
 * on where the page sits. The person scrolling, clicking or pressing a key
 * that moves the page ends the hold at once. Returns the way to end it.
 */
function holdInPlace(place: () => void, scroller: HTMLElement | null): () => void {
  let frame = 0;
  let observer: ResizeObserver | undefined;
  const started = performance.now();
  const onKey = (event: KeyboardEvent) => {
    if (!QUIET_KEYS.has(event.key)) stop();
  };
  const stop = () => {
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    observer?.disconnect();
    window.removeEventListener('wheel', stop, true);
    window.removeEventListener('touchmove', stop, true);
    window.removeEventListener('pointerdown', stop, true);
    window.removeEventListener('keydown', onKey, true);
  };
  const tick = () => {
    place();
    frame = performance.now() - started < SETTLE_MS ? requestAnimationFrame(tick) : 0;
    if (!frame) stop();
  };
  frame = requestAnimationFrame(tick);
  if (scroller && typeof ResizeObserver !== 'undefined') {
    observer = new ResizeObserver(place);
    observer.observe(scroller);
    for (const child of Array.from(scroller.children)) observer.observe(child);
  }
  window.addEventListener('wheel', stop, { capture: true, passive: true });
  window.addEventListener('touchmove', stop, { capture: true, passive: true });
  window.addEventListener('pointerdown', stop, { capture: true, passive: true });
  window.addEventListener('keydown', onKey, { capture: true, passive: true });
  return stop;
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
 *
 * An order that changes without a capture, as when a signed-in user's
 * saved order arrives after the frames have drawn, or another tab changes
 * it, asks `unplanned` how to treat the page. It is given the first card
 * shown before the change (`firstId` as it was), so the caller can tell a
 * view still at rest, move its cursor to the new first card, and answer
 * `rest`. Without it, or when it answers `anchor`, the current card is
 * brought into view if the change left it off screen.
 */
export function useReorderAnchor(
  order: string,
  findCurrent: () => HTMLElement | null,
  unplanned?: { firstId: number | null; decide: (previousFirstId: number | null) => ReorderMode },
) {
  const pendingRef = useRef<{ mode: 'anchor'; offset: number | null } | { mode: 'rest' } | null>(null);
  const orderRef = useRef(order);
  const findRef = useRef(findCurrent);
  findRef.current = findCurrent;
  const decideRef = useRef(unplanned?.decide);
  decideRef.current = unplanned?.decide;
  /** The first card as last drawn: before this commit's change. */
  const drawnFirstRef = useRef<number | null>(unplanned?.firstId ?? null);
  const firstId = unplanned?.firstId ?? null;

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
    const pending = pendingRef.current
      ?? (decideRef.current?.(drawnFirstRef.current) === 'rest'
        ? { mode: 'rest' as const }
        : { mode: 'anchor' as const, offset: null });
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
    if (offset === null) {
      // Brought into view once; previews and lazy cards settle a frame later.
      const frame = requestAnimationFrame(place);
      return () => cancelAnimationFrame(frame);
    }
    return holdInPlace(place, findRef.current()?.closest<HTMLElement>(SCROLLER) ?? null);
  }, [order]);

  // After the effect above, so it reads the first card from before.
  useLayoutEffect(() => {
    drawnFirstRef.current = firstId;
  }, [firstId]);

  return capture;
}
