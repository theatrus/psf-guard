/**
 * How much of the window the toolbar and the docked block may cover
 * together. Past STOP the block stops docking; it docks again only below
 * RESUME, so a block near the line does not flip at every small change.
 */
export const DOCK_SHARE = { stop: 0.55, resume: 0.5 } as const;

/** Whether the block is too tall to dock, given whether it was and the
 *  share of the window it and the toolbar now cover. */
export function nextTooTall(tooTall: boolean, share: number): boolean {
  return tooTall ? share >= DOCK_SHARE.resume : share > DOCK_SHARE.stop;
}

/**
 * The room under the header for the page, as it is with the header in
 * sight. On a phone the header slides away as the page scrolls down
 * (useTuckAway) and the page grows into its place; measuring with it in
 * sight keeps the docking from following the scroll's direction.
 */
export function pageRoom(scroller: HTMLElement | null): number {
  if (!scroller) return window.innerHeight;
  const app = scroller.parentElement;
  const header = app?.querySelector<HTMLElement>(':scope > .app-header');
  if (!app || !header) return scroller.clientHeight;
  const shown = header.getBoundingClientRect().bottom - app.getBoundingClientRect().top;
  return scroller.clientHeight - Math.max(0, header.offsetHeight - shown);
}
