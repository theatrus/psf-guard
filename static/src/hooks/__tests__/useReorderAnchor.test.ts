import { describe, expect, it } from 'vitest';
import { cardIsOnScreen, viewIsAtRest } from '../useReorderAnchor';

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
