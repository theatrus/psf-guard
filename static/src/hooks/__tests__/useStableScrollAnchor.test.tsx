import { act, render } from '@testing-library/react';
import { useLayoutEffect, useRef } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useStableScrollAnchor } from '../useStableScrollAnchor';

let notifyResize: (() => void) | undefined;

function rect(top: number, bottom: number): DOMRect {
  return {
    x: 0,
    y: top,
    top,
    bottom,
    left: 0,
    right: 800,
    width: 800,
    height: bottom - top,
    toJSON: () => ({}),
  };
}

function Harness({ shift }: { shift: number }) {
  const contentRef = useRef<HTMLDivElement>(null);
  const initialized = useRef(false);

  useLayoutEffect(() => {
    const content = contentRef.current!;
    const scroller = content.parentElement!;
    const controls = content.querySelector<HTMLElement>('.image-controls')!;
    const anchor = content.querySelector<HTMLElement>('[data-scroll-anchor]')!;

    if (!initialized.current) {
      scroller.scrollTop = 100;
      initialized.current = true;
    }
    scroller.getBoundingClientRect = () => rect(0, 600);
    controls.getBoundingClientRect = () => rect(0, 50);
    anchor.getBoundingClientRect = () => {
      const top = 200 + shift - (scroller.scrollTop - 100);
      return rect(top, top + 60);
    };
  }, [shift]);

  useStableScrollAnchor(contentRef, true);

  return (
    <main className="app-main">
      <div ref={contentRef}>
        <div className="image-controls sticky" />
        <div data-scroll-anchor="group:stable">Anchor</div>
      </div>
    </main>
  );
}

describe('useStableScrollAnchor', () => {
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

  it('offsets growth above the first visible anchor', () => {
    const view = render(<Harness shift={0} />);
    const scroller = view.container.querySelector<HTMLElement>('.app-main')!;

    view.rerender(<Harness shift={300} />);
    act(() => notifyResize?.());

    expect(scroller.scrollTop).toBe(400);
  });
});
