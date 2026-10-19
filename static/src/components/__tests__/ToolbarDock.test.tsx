import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/react';
import ToolbarDock from '../ToolbarDock';
import { nextTooTall } from '../../utils/toolbarDock';

// jsdom lays nothing out: these give the header, the toolbar, the dock and
// the scrolling region the sizes a browser would, and drive the observers.

const sizes = { header: 150, tucked: false, toolbar: 100, dock: 200, room: 800 };

let resize: () => void = () => {};
let intersect: (entry: Partial<IntersectionObserverEntry>) => void = () => {};

function renderDock(onCoverChange = vi.fn()) {
  const view = render(
    <div className="app">
      <header className="app-header" />
      <main className="app-main">
        <div>
          <section className="view-toolbar sticky" />
          <ToolbarDock onCoverChange={onCoverChange}>
            <p>chart</p>
          </ToolbarDock>
        </div>
      </main>
    </div>,
  );
  return { dock: view.container.querySelector<HTMLElement>('.toolbar-dock')!, onCoverChange };
}

/** The sentinel passes under the toolbar: the block has stuck. */
const stick = () => act(() => intersect({
  isIntersecting: false,
  boundingClientRect: { top: -10 } as DOMRectReadOnly,
  rootBounds: { top: 100 } as DOMRectReadOnly,
}));

beforeEach(() => {
  Object.assign(sizes, { header: 150, tucked: false, toolbar: 100, dock: 200, room: 800 });
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
    if (this.classList.contains('toolbar-dock')) return sizes.dock;
    if (this.classList.contains('app-header')) return sizes.header;
    return 0;
  });
  // A tucked header gives the page its room.
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (this: HTMLElement) {
    return this.classList.contains('app-main') ? sizes.room + (sizes.tucked ? sizes.header : 0) : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('view-toolbar')) return { top: 0, bottom: sizes.toolbar, height: sizes.toolbar } as DOMRect;
    if (this.classList.contains('app-header')) {
      const bottom = sizes.tucked ? 0 : sizes.header;
      return { top: bottom - sizes.header, bottom, height: sizes.header } as DOMRect;
    }
    return { top: 0, bottom: 0, height: 0 } as DOMRect;
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('nextTooTall', () => {
  it('stops docking past 55% and docks again only below 50%', () => {
    expect(nextTooTall(false, 0.54)).toBe(false);
    expect(nextTooTall(false, 0.56)).toBe(true);
    expect(nextTooTall(true, 0.52)).toBe(true);
    expect(nextTooTall(true, 0.49)).toBe(false);
  });
});

describe('ToolbarDock', () => {
  it('stops under the toolbar and is docked once the sentinel passes under it', () => {
    const { dock } = renderDock();
    expect(dock.style.getPropertyValue('--toolbar-dock-top')).toBe('100px');
    expect(dock).not.toHaveAttribute('data-docked');
    stick();
    expect(dock).toHaveAttribute('data-docked', 'true');
  });

  it('stops docking on a short window, and docks again only once well clear', () => {
    const { dock } = renderDock();
    stick();
    // A phone turned on its side: 300 of 500 is too much.
    sizes.room = 500;
    resize();
    expect(dock).toHaveAttribute('data-too-tall', 'true');
    expect(dock).not.toHaveAttribute('data-docked');
    // 300 of 570 is under 55% but not under 50%: still too tall.
    sizes.room = 570;
    resize();
    expect(dock).toHaveAttribute('data-too-tall', 'true');
    sizes.room = 800;
    resize();
    expect(dock).not.toHaveAttribute('data-too-tall');
    expect(dock).toHaveAttribute('data-docked', 'true');
  });

  it('does not follow the header sliding away and back', () => {
    // 460 of 800 with the header in sight; 460 of 950 were it measured
    // with the header tucked away.
    sizes.dock = 360;
    const { dock } = renderDock();
    stick();
    expect(dock).toHaveAttribute('data-too-tall', 'true');
    sizes.tucked = true;
    resize();
    expect(dock).toHaveAttribute('data-too-tall', 'true');
    sizes.tucked = false;
    resize();
    expect(dock).toHaveAttribute('data-too-tall', 'true');
  });

  it('reports a change of what it covers only once the page is scrolled past it', () => {
    const { dock, onCoverChange } = renderDock();
    sizes.dock = 220;
    resize();
    expect(onCoverChange).not.toHaveBeenCalled();
    stick();
    sizes.dock = 260;
    resize();
    expect(onCoverChange).toHaveBeenCalledTimes(1);
    // The window changing alone is not the block changing.
    sizes.room = 820;
    resize();
    expect(onCoverChange).toHaveBeenCalledTimes(1);
    // Too tall, it still moves the page under it as it changes size.
    sizes.dock = 500;
    resize();
    expect(dock).toHaveAttribute('data-too-tall', 'true');
    expect(onCoverChange).toHaveBeenCalledTimes(2);
    // Shrinking to fit, it docks again: once for the size, once for docking.
    sizes.dock = 200;
    resize();
    expect(dock).toHaveAttribute('data-docked', 'true');
    expect(onCoverChange).toHaveBeenCalledTimes(4);
  });
});
