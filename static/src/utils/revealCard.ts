/** Room left between the covers or the bottom and a card brought into sight. */
const GAP = 8;
/** Frames a card must hold still after a scroll before the checks stop. */
const STILL_FRAMES = 4;
/** The most frames to keep checking for. */
const MAX_CHECKS = 20;

/**
 * Scrolls a Sequence strip's current card into sight, as scrollIntoView's
 * `nearest` does, but measured from below what covers the top of the page:
 * the sticky toolbar, the docked chart beside the strip, and a suspect list
 * hanging below the chart while it is docked. (Not docked, the list moves
 * with the page, and scrolling cannot clear a card under it.) True when it
 * had to scroll.
 */
export interface RevealOptions {
  /** Leave a card wholly outside the scrolling region where it is: after
   *  the covers change, only a card that was in the window is kept there,
   *  and one the person scrolled away from stays away. */
  onlyInWindow?: boolean;
}

export function revealCard(strip: HTMLElement, { onlyInWindow = false }: RevealOptions = {}): boolean {
  const card = strip.querySelector<HTMLElement>('.sequence-image-card.current-selection');
  const scroller = strip.closest<HTMLElement>('.app-main');
  if (!card || !scroller) return false;
  const cardRect = card.getBoundingClientRect();
  const scrollerRect = scroller.getBoundingClientRect();
  if (onlyInWindow && (cardRect.bottom <= scrollerRect.top || cardRect.top >= scrollerRect.bottom)) {
    return false;
  }
  const covers = strip.parentElement?.querySelectorAll<HTMLElement>(
    ':scope > .view-toolbar.sticky, :scope > .toolbar-dock, :scope > .toolbar-dock[data-docked] .sequence-issue-list',
  ) ?? [];
  const visibleTop = Math.max(
    scrollerRect.top,
    ...Array.from(covers, cover => cover.getBoundingClientRect().bottom),
  );
  const isClipped = cardRect.top < visibleTop - 1 || cardRect.bottom > scrollerRect.bottom + 1;
  const isOutsideViewport = cardRect.bottom <= visibleTop || cardRect.top >= scrollerRect.bottom;
  const fitsViewport = cardRect.height <= scrollerRect.bottom - visibleTop;
  if (!isOutsideViewport && !(fitsViewport && isClipped)) return false;
  scroller.scrollTop += cardRect.top < visibleTop || !fitsViewport
    ? cardRect.top - visibleTop - GAP
    : cardRect.bottom - scrollerRect.bottom + GAP;
  return true;
}

/**
 * Reveals the strip's current card from the next frame on, and returns a
 * function that stops.
 *
 * Cards lay themselves out as they come into sight (content-visibility), a
 * frame or two after the scroll that brought them, and Safari has no scroll
 * anchoring to make up for it: they can move the card again. After a
 * scroll, this looks again until the card has held still for a few frames.
 * The person's own scroll, by wheel, touch, scrollbar or key, ends it, so
 * it never pulls the page back from them.
 */
export function revealCardUntilStill(strip: HTMLElement, options: RevealOptions = {}): () => void {
  const scroller = strip.closest<HTMLElement>('.app-main');
  let frame = 0;
  let checks = 0;
  let still = 0;
  const stop = () => {
    cancelAnimationFrame(frame);
    scroller?.removeEventListener('wheel', stop);
    scroller?.removeEventListener('touchstart', stop);
    scroller?.removeEventListener('pointerdown', stop);
    window.removeEventListener('keydown', stop, true);
  };
  const check = () => {
    checks += 1;
    still = revealCard(strip, options) ? 0 : still + 1;
    if (still < STILL_FRAMES && checks < MAX_CHECKS) frame = requestAnimationFrame(check);
    else stop();
  };
  frame = requestAnimationFrame(() => {
    if (revealCard(strip, options)) frame = requestAnimationFrame(check);
    else stop();
  });
  scroller?.addEventListener('wheel', stop, { passive: true });
  scroller?.addEventListener('touchstart', stop, { passive: true });
  scroller?.addEventListener('pointerdown', stop);
  window.addEventListener('keydown', stop, true);
  return stop;
}
