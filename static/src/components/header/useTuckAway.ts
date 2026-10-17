import { useEffect, useRef, useState, type RefObject } from 'react';

/** Only a phone tucks the header away: elsewhere it keeps its place. */
const PHONE_QUERY = '(max-width: 760px)';
/** How far the page scrolls one way before the header leaves or returns. */
export const TRAVEL_PX = 24;
/** Near the top of the page the header always shows. */
export const TOP_PX = 48;
/** The header's move changes the page's height under the scroll, and the
 *  browser may move the scroll to fit; that is not the person scrolling. */
export const SETTLE_MS = 260;

export interface TuckState {
  tucked: boolean;
  /** Where the scroll last turned, or settled. */
  anchor: number;
}

/**
 * The next state for a scroll to `top`. Scrolling down past the top tucks
 * the header away once the page has travelled `TRAVEL_PX`; scrolling up as
 * far brings it back. A page with less room below than the header's height
 * keeps it: tucking would grow the page, pull the scroll back and bring the
 * header straight back.
 */
export function nextTuck(
  state: TuckState,
  top: number,
  roomBelow: number,
  headerHeight: number
): TuckState {
  if (top <= TOP_PX) return { tucked: false, anchor: top };
  const travel = top - state.anchor;
  if (!state.tucked) {
    if (travel < 0) return { tucked: false, anchor: top };
    if (travel >= TRAVEL_PX && roomBelow > headerHeight) return { tucked: true, anchor: top };
    return state;
  }
  if (travel > 0) return { tucked: true, anchor: top };
  if (-travel >= TRAVEL_PX) return { tucked: false, anchor: top };
  return state;
}

/**
 * On a phone, the header slides away while the page scrolls down and comes
 * back when it scrolls up, so the images and stacks get the screen. The
 * page scrolls in `.app-main`, beside the header. Keyboard focus in the
 * header, or another page, brings it back.
 *
 * The header's height goes on it as `--app-header-height`, which the
 * tucked style moves it up by.
 */
export function useTuckAway(header: RefObject<HTMLElement | null>, page: string): boolean {
  const [tucked, setTucked] = useState(false);
  const state = useRef<TuckState>({ tucked: false, anchor: 0 });

  // Another page starts with the header in sight.
  useEffect(() => {
    const scroller = header.current?.parentElement?.querySelector<HTMLElement>('.app-main');
    state.current = { tucked: false, anchor: scroller?.scrollTop ?? 0 };
    setTucked(false);
  }, [header, page]);

  useEffect(() => {
    const root = header.current;
    const scroller = root?.parentElement?.querySelector<HTMLElement>('.app-main');
    if (!root || !scroller || typeof window.matchMedia !== 'function') return;
    const phone = window.matchMedia(PHONE_QUERY);
    let settleUntil = 0;
    let height = root.offsetHeight;

    const apply = (next: TuckState) => {
      if (next.tucked !== state.current.tucked) settleUntil = performance.now() + SETTLE_MS;
      state.current = next;
      setTucked(next.tucked);
    };
    const onScroll = () => {
      const top = scroller.scrollTop;
      if (!phone.matches) {
        if (state.current.tucked) apply({ tucked: false, anchor: top });
        return;
      }
      if (performance.now() < settleUntil) {
        state.current = { ...state.current, anchor: top };
        return;
      }
      const roomBelow = scroller.scrollHeight - scroller.clientHeight - top;
      const next = nextTuck(state.current, top, roomBelow, height);
      if (next !== state.current) apply(next);
    };
    const untuck = () => {
      if (state.current.tucked) apply({ tucked: false, anchor: scroller.scrollTop });
    };
    const onPhoneChange = () => {
      if (!phone.matches) untuck();
    };

    const measure = () => {
      height = root.offsetHeight;
      root.style.setProperty('--app-header-height', `${height}px`);
    };
    measure();
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure);
    observer?.observe(root);

    scroller.addEventListener('scroll', onScroll, { passive: true });
    root.addEventListener('focusin', untuck);
    phone.addEventListener?.('change', onPhoneChange);
    return () => {
      scroller.removeEventListener('scroll', onScroll);
      root.removeEventListener('focusin', untuck);
      phone.removeEventListener?.('change', onPhoneChange);
      observer?.disconnect();
    };
  }, [header]);

  return tucked;
}
