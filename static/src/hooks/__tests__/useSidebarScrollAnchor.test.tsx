import { act, render } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useSidebarScrollAnchor } from '../useSidebarScrollAnchor';

let notifyResize: (() => void) | undefined;

function rect(top: number, height: number): DOMRect {
  return {
    x: 0, y: top, top, bottom: top + height, left: 0, right: 360, width: 360, height,
    toJSON: () => ({}),
  };
}

const PANELS = ['Information', 'Calibration', 'Sky context', 'Satellites', 'Actions'];

/**
 * A sidebar 600 px tall holding the panels, laid out by hand: jsdom does no
 * layout. `heights` are the panels' heights; the sidebar's own top is 0.
 */
function Sidebar({ heights }: { heights: number[] }) {
  const [sidebar, setSidebar] = useState<HTMLDivElement | null>(null);
  useSidebarScrollAnchor(sidebar);
  return (
    <div
      data-testid="sidebar"
      ref={(element) => {
        if (!element) return;
        element.getBoundingClientRect = () => rect(0, 600);
        Object.defineProperty(element, 'scrollHeight', {
          configurable: true,
          get: () => heights.reduce((sum, height) => sum + height, 0),
        });
        Array.from(element.children).forEach((child, index) => {
          (child as HTMLElement).getBoundingClientRect = () => {
            const above = heights.slice(0, index).reduce((sum, height) => sum + height, 0);
            return rect(above - element.scrollTop, heights[index]);
          };
        });
        setSidebar(element);
      }}
    >
      {PANELS.slice(0, heights.length).map((name) => <section key={name}>{name}</section>)}
    </div>
  );
}

/** Where the Satellites panel sits from the top of the sidebar. */
function satellitesTop(sidebar: HTMLElement) {
  return sidebar.children[3].getBoundingClientRect().top;
}

describe('useSidebarScrollAnchor', () => {
  beforeEach(() => {
    notifyResize = undefined;
    vi.stubGlobal('ResizeObserver', class {
      constructor(callback: ResizeObserverCallback) {
        notifyResize = () => callback([], this as unknown as ResizeObserver);
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('keeps the panel being read in place when one above it opens', () => {
    const view = render(<Sidebar heights={[300, 40, 60, 400]} />);
    const sidebar = view.getByTestId('sidebar');
    // Reading the satellites: scrolled past the information panel.
    sidebar.scrollTop = 380;
    sidebar.dispatchEvent(new Event('scroll'));
    expect(satellitesTop(sidebar)).toBe(20);

    // The next light's Calibration panel opens: 40 → 500 px.
    view.rerender(<Sidebar heights={[300, 500, 60, 400]} />);
    act(() => notifyResize?.());
    expect(sidebar.scrollTop).toBe(840);
    expect(satellitesTop(sidebar)).toBe(20);
  });

  it('keeps the same panel through a panel above that shrinks while loading and grows back', () => {
    // Satellites 274 px down, under a long Sky context.
    const view = render(<Sidebar heights={[300, 40, 700, 400, 300]} />);
    const sidebar = view.getByTestId('sidebar');
    sidebar.scrollTop = 766;
    sidebar.dispatchEvent(new Event('scroll'));
    expect(satellitesTop(sidebar)).toBe(274);

    // Arrow key: Sky context shows only its heading while it reads the next
    // light. Calibration now starts in view, but the reader is still on
    // Satellites; the browser's scroll event for the adjustment changes nothing.
    view.rerender(<Sidebar heights={[300, 40, 50, 400, 300]} />);
    act(() => notifyResize?.());
    sidebar.dispatchEvent(new Event('scroll'));
    expect(satellitesTop(sidebar)).toBe(274);

    // The data arrives, and that light's Calibration opens too.
    view.rerender(<Sidebar heights={[300, 500, 700, 400, 300]} />);
    act(() => notifyResize?.());
    expect(satellitesTop(sidebar)).toBe(274);
    expect(sidebar.scrollTop).toBe(1226);
  });

  it('does not take the scroll clamped by shorter content for the reader', () => {
    const view = render(<Sidebar heights={[300, 40, 700, 400, 300]} />);
    const sidebar = view.getByTestId('sidebar');
    sidebar.scrollTop = 766;
    sidebar.dispatchEvent(new Event('scroll'));

    // Sky context collapses while loading: 1740 → 1090 px of content in a
    // 600 px sidebar. The browser clamps the scroll to 490 and reports it
    // before the resize does.
    view.rerender(<Sidebar heights={[300, 40, 50, 400, 300]} />);
    sidebar.scrollTop = 490;
    sidebar.dispatchEvent(new Event('scroll'));
    act(() => notifyResize?.());
    expect(satellitesTop(sidebar)).toBe(274);

    view.rerender(<Sidebar heights={[300, 500, 700, 400, 300]} />);
    act(() => notifyResize?.());
    expect(satellitesTop(sidebar)).toBe(274);
  });

  it('lets the reader scroll away from the anchor', () => {
    const view = render(<Sidebar heights={[300, 40, 700, 400, 300]} />);
    const sidebar = view.getByTestId('sidebar');
    sidebar.scrollTop = 766;
    sidebar.dispatchEvent(new Event('scroll'));
    // Back to the top, where nothing is anchored.
    sidebar.scrollTop = 0;
    sidebar.dispatchEvent(new Event('scroll'));

    view.rerender(<Sidebar heights={[300, 500, 700, 400, 300]} />);
    act(() => notifyResize?.());
    expect(sidebar.scrollTop).toBe(0);
  });

  it('leaves a sidebar at the top at the top', () => {
    const view = render(<Sidebar heights={[300, 40, 400]} />);
    const sidebar = view.getByTestId('sidebar');
    view.rerender(<Sidebar heights={[300, 500, 400]} />);
    act(() => notifyResize?.());
    expect(sidebar.scrollTop).toBe(0);
  });
});
