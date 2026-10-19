import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/react';
import ToolbarDock from '../ToolbarDock';

// jsdom lays nothing out: these give the toolbar, the dock and the
// scrolling region the sizes a browser would, and drive the observers.

const sizes = { toolbar: 100, dock: 200, room: 800 };
// Where the dock sits on screen: stuck under the toolbar, or, too tall to
// dock, where the page puts it, scrolled away above.
const placeOf = (dock: Element) => (dock.hasAttribute('data-too-tall') ? -300 : 100);

let resize: () => void = () => {};
let intersect: (entry: Partial<IntersectionObserverEntry>) => void = () => {};

function renderDock(onDockedResize = vi.fn()) {
  const view = render(
    <div className="app-main">
      <div>
        <section className="view-toolbar sticky" />
        <ToolbarDock onDockedResize={onDockedResize}><p>chart</p></ToolbarDock>
      </div>
    </div>,
  );
  const scroller = view.container.querySelector<HTMLElement>('.app-main')!;
  let scrollTop = 1000;
  Object.defineProperty(scroller, 'scrollTop', {
    get: () => scrollTop,
    set: (value: number) => { scrollTop = value; },
    configurable: true,
  });
  return { dock: view.container.querySelector<HTMLElement>('.toolbar-dock')!, scroller, onDockedResize };
}

/** The sentinel passes under the toolbar: the block has stuck. */
const stick = () => act(() => intersect({
  isIntersecting: false,
  boundingClientRect: { top: -10 } as DOMRectReadOnly,
  rootBounds: { top: 100 } as DOMRectReadOnly,
}));

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) { resize = () => act(() => callback()); }
    observe() {}
    disconnect() {}
  });
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: IntersectionObserverCallback) {
      intersect = (entry) => callback([entry as IntersectionObserverEntry], this as unknown as IntersectionObserver);
    }
    observe() {}
    disconnect() {}
  });
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
    return this.classList.contains('toolbar-dock') ? sizes.dock : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (this: HTMLElement) {
    return this.classList.contains('app-main') ? sizes.room : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('view-toolbar')) return { top: 0, bottom: sizes.toolbar, height: sizes.toolbar } as DOMRect;
    if (this.classList.contains('toolbar-dock')) {
      const top = placeOf(this);
      return { top, bottom: top + sizes.dock, height: sizes.dock } as DOMRect;
    }
    return { top: 0, bottom: 0, height: 0 } as DOMRect;
  });
  Object.assign(sizes, { toolbar: 100, dock: 200, room: 800 });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('ToolbarDock', () => {
  it('stops under the toolbar and is docked once the sentinel passes under it', () => {
    const { dock } = renderDock();
    expect(dock.style.getPropertyValue('--toolbar-dock-top')).toBe('100px');
    expect(dock).not.toHaveAttribute('data-docked');
    stick();
    expect(dock).toHaveAttribute('data-docked', 'true');
  });

  it('does not dock when it and the toolbar would cover too much of the window', () => {
    const { dock } = renderDock();
    stick();
    // A phone turned on its side: 300 of 450 is too much.
    sizes.room = 450;
    resize();
    expect(dock).toHaveAttribute('data-too-tall', 'true');
    expect(dock).not.toHaveAttribute('data-docked');
    // Turned back, it docks again.
    sizes.room = 800;
    resize();
    expect(dock).not.toHaveAttribute('data-too-tall');
    expect(dock).toHaveAttribute('data-docked', 'true');
  });

  it('keeps its place on screen when it grows too tall while docked', () => {
    const { dock, scroller } = renderDock();
    stick();
    sizes.dock = 400;
    resize();
    expect(dock).toHaveAttribute('data-too-tall', 'true');
    // It was at 100 and the page put it at -300: the page scrolls up 400.
    expect(scroller.scrollTop).toBe(600);
  });

  it('reports a change of size only while docked', () => {
    const { onDockedResize } = renderDock();
    sizes.dock = 220;
    resize();
    expect(onDockedResize).not.toHaveBeenCalled();
    stick();
    sizes.dock = 260;
    resize();
    expect(onDockedResize).toHaveBeenCalledTimes(1);
    // The toolbar or the window changing alone is not the block resizing.
    sizes.room = 900;
    resize();
    expect(onDockedResize).toHaveBeenCalledTimes(1);
  });
});
