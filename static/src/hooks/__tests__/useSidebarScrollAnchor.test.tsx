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

/**
 * A sidebar 600 px tall holding three panels, laid out by hand: jsdom does
 * no layout. `heights` are the panels' heights; the sidebar's own top is 0.
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
        Array.from(element.children).forEach((child, index) => {
          (child as HTMLElement).getBoundingClientRect = () => {
            const above = heights.slice(0, index).reduce((sum, height) => sum + height, 0);
            return rect(above - element.scrollTop, heights[index]);
          };
        });
        setSidebar(element);
      }}
    >
      <section>Information</section>
      <section>Calibration</section>
      <section>Satellites</section>
    </div>
  );
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
    const view = render(<Sidebar heights={[300, 40, 400]} />);
    const sidebar = view.getByTestId('sidebar');
    // Reading the satellites: scrolled past the information panel.
    sidebar.scrollTop = 320;
    sidebar.dispatchEvent(new Event('scroll'));

    // The next light's Calibration panel opens: 40 → 500 px.
    view.rerender(<Sidebar heights={[300, 500, 400]} />);
    act(() => notifyResize?.());
    expect(sidebar.scrollTop).toBe(780);
  });

  it('leaves a sidebar at the top at the top', () => {
    const view = render(<Sidebar heights={[300, 40, 400]} />);
    const sidebar = view.getByTestId('sidebar');
    view.rerender(<Sidebar heights={[300, 500, 400]} />);
    act(() => notifyResize?.());
    expect(sidebar.scrollTop).toBe(0);
  });
});
