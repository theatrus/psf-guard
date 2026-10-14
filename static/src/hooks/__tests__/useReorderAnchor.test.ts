import { afterEach, describe, expect, it } from 'vitest';
import { act, fireEvent, renderHook, waitFor } from '@testing-library/react';
import { cardIsOnScreen, SETTLE_MS, useReorderAnchor, viewIsAtRest } from '../useReorderAnchor';

describe('holding the current card after a flip', () => {
  afterEach(() => {
    document.body.innerHTML = '';
  });

  /** A scrolling region 800 px tall, and the current card in it, `above`
   *  px of content below the region's top edge before scrolling. */
  function page() {
    const scroller = document.createElement('main');
    scroller.className = 'app-main';
    const card = document.createElement('div');
    scroller.appendChild(card);
    document.body.appendChild(scroller);
    let scrollTop = 0;
    Object.defineProperty(scroller, 'scrollTop', {
      get: () => scrollTop,
      set: (value: number) => { scrollTop = value; },
    });
    const layout = { above: 1000 };
    scroller.getBoundingClientRect = () => ({ top: 0, bottom: 800 } as DOMRect);
    card.getBoundingClientRect = () => {
      const top = layout.above - scrollTop;
      return { top, bottom: top + 300 } as DOMRect;
    };
    return { scroller, card, layout, top: () => card.getBoundingClientRect().top };
  }

  it('keeps it at its height while lazy cards settle, and lets go when the person moves', async () => {
    const { scroller, card, layout, top } = page();
    scroller.scrollTop = 700;
    expect(top()).toBe(300);
    const { result, rerender } = renderHook(
      ({ order }) => useReorderAnchor(order, () => card),
      { initialProps: { order: 'oldest' } },
    );

    act(() => result.current('anchor'));
    // The flip moves the card far down the page; it goes back to 300 px.
    layout.above = 2000;
    rerender({ order: 'newest' });
    expect(top()).toBe(300);

    // A card above it then draws 18 px shorter than the room it held.
    layout.above = 1982;
    await waitFor(() => expect(top()).toBe(300));
    // Closing the View panel is not a move.
    fireEvent.keyDown(window, { key: 'Escape' });
    layout.above = 1970;
    await waitFor(() => expect(top()).toBe(300));

    // An arrow key is: the page is the person's again.
    fireEvent.keyDown(window, { key: 'ArrowDown' });
    layout.above = 1900;
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(top()).toBe(230);
  });

  it('lets go by itself once the page has settled', async () => {
    const { scroller, card, layout, top } = page();
    scroller.scrollTop = 700;
    const { result, rerender } = renderHook(
      ({ order }) => useReorderAnchor(order, () => card),
      { initialProps: { order: 'oldest' } },
    );
    act(() => result.current('anchor'));
    layout.above = 2000;
    rerender({ order: 'newest' });
    await new Promise((resolve) => setTimeout(resolve, SETTLE_MS + 200));
    layout.above = 1950;
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(top()).toBe(250);
  });
});

describe('a view at rest', () => {
  const rest = { currentId: 1, firstId: 1, touched: false, firstOnScreen: true };

  it('is one whose cursor is still on the first card, unmoved and in sight', () => {
    expect(viewIsAtRest(rest)).toBe(true);
  });

  it('is not once the person moved the cursor, even back to the first card', () => {
    expect(viewIsAtRest({ ...rest, touched: true })).toBe(false);
  });

  it('is not when the cursor sits on another card, as a link can leave it', () => {
    expect(viewIsAtRest({ ...rest, currentId: 2 })).toBe(false);
  });

  it('is not when the first card was scrolled out of sight', () => {
    expect(viewIsAtRest({ ...rest, firstOnScreen: false })).toBe(false);
  });

  it('is not with nothing to show', () => {
    expect(viewIsAtRest({ ...rest, currentId: null, firstId: null })).toBe(false);
  });
});

describe('a card on screen', () => {
  function layout(cardTop: number, cardBottom: number) {
    const scroller = document.createElement('main');
    scroller.className = 'app-main';
    const card = document.createElement('div');
    scroller.appendChild(card);
    scroller.getBoundingClientRect = () => ({ top: 100, bottom: 900 } as DOMRect);
    card.getBoundingClientRect = () => ({ top: cardTop, bottom: cardBottom } as DOMRect);
    return card;
  }

  it('shows when any of it is inside the scrolling region', () => {
    expect(cardIsOnScreen(layout(120, 400))).toBe(true);
    expect(cardIsOnScreen(layout(50, 150))).toBe(true);
  });

  it('does not when it is above or below it', () => {
    expect(cardIsOnScreen(layout(-300, 100))).toBe(false);
    expect(cardIsOnScreen(layout(900, 1200))).toBe(false);
  });

  it('counts as shown with no layout to measure', () => {
    expect(cardIsOnScreen(document.createElement('div'))).toBe(true);
    expect(cardIsOnScreen(null)).toBe(true);
  });
});
