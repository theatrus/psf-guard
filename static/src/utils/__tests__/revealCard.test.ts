import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { revealCard, revealCardUntilStill } from '../revealCard';

// A strip beside a sticky toolbar and a docked chart covering the top
// 300px of a 1000px scrolling region. Cards move with the scroll.

let scrollTop = 0;
let cardTop = 0;
let frames: FrameRequestCallback[] = [];

function setup() {
  document.body.innerHTML = `
    <div class="app-main">
      <div class="sequence-view">
        <section class="view-toolbar sticky"></section>
        <div class="toolbar-dock"></div>
        <div class="sequence-strip"><div class="sequence-image-card current-selection"></div></div>
      </div>
    </div>`;
  const scroller = document.querySelector<HTMLElement>('.app-main')!;
  Object.defineProperty(scroller, 'scrollTop', {
    get: () => scrollTop,
    set: (value: number) => { cardTop -= value - scrollTop; scrollTop = value; },
    configurable: true,
  });
  return { strip: document.querySelector<HTMLElement>('.sequence-strip')!, scroller };
}

/** Run the frames requested so far, as the browser would draw one. */
function nextFrame() {
  const pending = frames;
  frames = [];
  pending.forEach(callback => callback(0));
}

beforeEach(() => {
  scrollTop = 1000;
  frames = [];
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('app-main')) return { top: 0, bottom: 1000, height: 1000 } as DOMRect;
    if (this.classList.contains('view-toolbar')) return { top: 0, bottom: 100, height: 100 } as DOMRect;
    if (this.classList.contains('toolbar-dock')) return { top: 100, bottom: 300, height: 200 } as DOMRect;
    if (this.classList.contains('sequence-image-card')) return { top: cardTop, bottom: cardTop + 200, height: 200 } as DOMRect;
    return { top: 0, bottom: 0, height: 0 } as DOMRect;
  });
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => frames.push(callback));
  vi.stubGlobal('cancelAnimationFrame', () => { frames = []; });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.innerHTML = '';
});

describe('revealCard', () => {
  it('brings a card under the docked chart to just below it', () => {
    const { strip } = setup();
    cardTop = 150;
    expect(revealCard(strip)).toBe(true);
    expect(cardTop).toBe(308);
    expect(revealCard(strip)).toBe(false);
  });

  it('brings a card below the window up to its bottom edge', () => {
    const { strip } = setup();
    cardTop = 900;
    expect(revealCard(strip)).toBe(true);
    expect(cardTop + 200).toBe(992);
  });

  it('after the covers change, leaves a card the person scrolled away from', () => {
    const { strip } = setup();
    cardTop = 1500;
    expect(revealCard(strip, { onlyInWindow: true })).toBe(false);
    expect(scrollTop).toBe(1000);
    // One still partly in the window, under the chart, is brought out.
    cardTop = 150;
    expect(revealCard(strip, { onlyInWindow: true })).toBe(true);
    expect(cardTop).toBe(308);
  });
});

describe('revealCardUntilStill', () => {
  it('looks again while cards laid out late move the card', () => {
    const { strip } = setup();
    cardTop = 1500;
    revealCardUntilStill(strip);
    nextFrame();
    expect(cardTop).toBe(792);
    // Cards above lay themselves out and push it under the chart.
    cardTop = 200;
    nextFrame();
    expect(cardTop).toBe(308);
    for (let frame = 0; frame < 4; frame += 1) nextFrame();
    expect(frames).toHaveLength(0);
  });

  it("stops at the person's own scroll", () => {
    const { strip, scroller } = setup();
    cardTop = 1500;
    revealCardUntilStill(strip);
    nextFrame();
    scroller.dispatchEvent(new Event('wheel'));
    cardTop = 200;
    nextFrame();
    expect(cardTop).toBe(200);
    expect(frames).toHaveLength(0);
  });

  it('does nothing more when the card is already in sight', () => {
    const { strip } = setup();
    cardTop = 400;
    revealCardUntilStill(strip);
    nextFrame();
    expect(scrollTop).toBe(1000);
    expect(frames).toHaveLength(0);
  });
});
