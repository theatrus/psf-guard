import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { useImageZoom } from '../useImageZoom';

// Container mocked at 820x620 → fit padding (-20) leaves 800x600.
const CONTAINER = { width: 820, height: 620 };

function makeContainer(): HTMLDivElement {
  const el = document.createElement('div');
  el.getBoundingClientRect = () =>
    ({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: CONTAINER.width,
      bottom: CONTAINER.height,
      width: CONTAINER.width,
      height: CONTAINER.height,
      toJSON: () => ({}),
    }) as DOMRect;
  return el;
}

function makeImg(naturalWidth: number, naturalHeight: number): HTMLImageElement {
  // complete:false keeps the hook's mount auto-fit effect out of the way so
  // each test drives state transitions explicitly.
  return { naturalWidth, naturalHeight, complete: false } as HTMLImageElement;
}

function setup(onViewModeChange?: (mode: 'fit' | 'user') => void) {
  const hook = renderHook(() =>
    useImageZoom({ minScale: 0.1, maxScale: 10.0, onViewModeChange })
  );
  hook.result.current.containerRef.current = makeContainer();
  return hook;
}

function panBy(
  result: { current: ReturnType<typeof useImageZoom> },
  dx: number,
  dy: number
) {
  const down = {
    button: 0,
    clientX: 400,
    clientY: 300,
    preventDefault: () => {},
  } as React.MouseEvent;
  const move = {
    clientX: 400 + dx,
    clientY: 300 + dy,
    preventDefault: () => {},
  } as React.MouseEvent;
  act(() => {
    result.current.handleMouseDown(down);
    result.current.handleMouseMove(move);
    result.current.handleMouseUp(move);
  });
}

function touchEvent(points: Array<{ x: number; y: number }>) {
  const preventDefault = vi.fn();
  return {
    event: {
      touches: points.map(({ x, y }) => ({ clientX: x, clientY: y })),
      preventDefault,
    } as unknown as React.TouchEvent,
    preventDefault,
  };
}

describe('useImageZoom applyBitmapDimensions', () => {
  it("fit mode centers the bitmap at the container's fit scale", () => {
    const { result } = setup();

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
    });

    // fit = min(800/2000, 600/1333) = 0.4, centered.
    expect(result.current.zoomState.scale).toBeCloseTo(0.4, 5);
    expect(result.current.zoomState.offsetX).toBeCloseTo(
      (CONTAINER.width - 2000 * 0.4) / 2,
      5
    );
    expect(result.current.zoomState.offsetY).toBeCloseTo(
      (CONTAINER.height - 1333 * 0.4) / 2,
      5
    );
  });

  it('preserve mode keeps the state EXACTLY when dimensions match (arrow-key navigation)', () => {
    const { result } = setup();

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
    });
    act(() => {
      result.current.zoomIn();
    });
    panBy(result, -120, -80);
    const before = result.current.zoomState;

    // Next image in the sequence loads with identical bitmap dimensions.
    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'preserve');
    });

    expect(result.current.zoomState.scale).toBe(before.scale);
    expect(result.current.zoomState.offsetX).toBe(before.offsetX);
    expect(result.current.zoomState.offsetY).toBe(before.offsetY);
  });

  it('preserve mode keeps displayed size and center across a bitmap size change', () => {
    const { result } = setup();

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
    });
    // Zoom to raw 1.0 on the preview bitmap.
    act(() => result.current.zoomIn());
    act(() => result.current.zoomIn());
    act(() => result.current.zoomIn());
    const before = result.current.zoomState;
    expect(before.scale).toBeCloseTo(1.0, 5);

    const beforeDisplayedWidth = 2000 * before.scale;
    const beforeCenterRatioX =
      (CONTAINER.width / 2 - before.offsetX) / before.scale / 2000;
    const beforeCenterRatioY =
      (CONTAINER.height / 2 - before.offsetY) / before.scale / 1333;

    // The original-resolution bitmap (3x) replaces the preview.
    act(() => {
      result.current.applyBitmapDimensions(6000, 3999, 'preserve');
    });

    const after = result.current.zoomState;
    expect(6000 * after.scale).toBeCloseTo(beforeDisplayedWidth, 3);
    expect(
      (CONTAINER.width / 2 - after.offsetX) / after.scale / 6000
    ).toBeCloseTo(beforeCenterRatioX, 5);
    expect(
      (CONTAINER.height / 2 - after.offsetY) / after.scale / 3999
    ).toBeCloseTo(beforeCenterRatioY, 5);
  });

  it('constrains pans against the state-calibrated dims, not a stale <img> (top-left jump regression)', () => {
    const { result } = setup();

    // The <img> element still holds the OLD preview bitmap for the whole
    // swap window — constraints must ignore it.
    result.current.imageRef.current = makeImg(2000, 1333);

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
    });
    act(() => result.current.zoomIn());
    act(() => result.current.zoomIn());
    act(() => result.current.zoomIn());
    act(() => {
      result.current.applyBitmapDimensions(6000, 3999, 'preserve');
    });
    // State is now calibrated to 6000x3999 at scale ~1/3; the stale <img>
    // (2000px) would make the scaled size ~667px < container and every
    // constraint recenters toward the small-image bounds.
    const scale = result.current.zoomState.scale;
    expect(6000 * scale).toBeGreaterThan(CONTAINER.width);

    panBy(result, -500, -300);

    const { offsetX, offsetY } = result.current.zoomState;
    // Against the correct dims the pan lands well inside bounds; against the
    // stale <img> dims it would have been recentered to (76.7, 88.4).
    expect(offsetX).toBeLessThan(-400);
    expect(offsetY).toBeLessThan(-200);
  });

  it('zoomToFit without a loaded <img> falls back to the state dims instead of a top-left reset', () => {
    const { result } = setup();

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
    });
    act(() => result.current.zoomIn());
    const zoomed = result.current.zoomState;
    expect(zoomed.scale).toBeGreaterThan(0.4);

    // No usable <img> (src swap in flight) — fit must still target the
    // calibrated bitmap, not collapse to scale 1 at (0,0).
    act(() => {
      result.current.zoomToFit();
    });

    expect(result.current.zoomState.scale).toBeCloseTo(0.4, 5);
    expect(result.current.zoomState.offsetX).toBeCloseTo(
      (CONTAINER.width - 2000 * 0.4) / 2,
      5
    );
  });
});

describe('useImageZoom notifyBitmapDimensions', () => {
  it('recalibrates pan constraints to the loaded bitmap without touching the transform', () => {
    const { result } = setup();

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
    });
    act(() => result.current.zoomIn());
    act(() => result.current.zoomIn());
    act(() => result.current.zoomIn());
    const before = result.current.zoomState;

    // Comparison-view preserved-zoom navigation: a LARGER bitmap loads but
    // nothing adjusts the transform — only the calibration must move over.
    act(() => {
      result.current.notifyBitmapDimensions(6000, 3999);
    });

    expect(result.current.zoomState.scale).toBe(before.scale);
    expect(result.current.zoomState.offsetX).toBe(before.offsetX);
    expect(result.current.zoomState.offsetY).toBe(before.offsetY);

    // Pans must now clamp against the 6000px bitmap's bounds; against the
    // stale 2000px dims this pan would be clamped to ~(-1380, -846).
    panBy(result, -2500, -1500);
    expect(result.current.zoomState.offsetX).toBeLessThan(-2000);
    expect(result.current.zoomState.offsetY).toBeLessThan(-1000);
  });
});

describe('useImageZoom view-mode notifications', () => {
  it('reports user intent on zoom/pan and fit intent on zoomToFit', () => {
    const onViewModeChange = vi.fn();
    const { result } = setup(onViewModeChange);

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
    });
    // Programmatic fit/apply never notifies.
    expect(onViewModeChange).not.toHaveBeenCalled();

    act(() => result.current.zoomIn());
    expect(onViewModeChange).toHaveBeenLastCalledWith('user');

    panBy(result, -50, 0);
    expect(onViewModeChange).toHaveBeenLastCalledWith('user');

    act(() => result.current.zoomToFit());
    expect(onViewModeChange).toHaveBeenLastCalledWith('fit');

    act(() => result.current.zoomTo100());
    expect(onViewModeChange).toHaveBeenLastCalledWith('user');
  });
});

describe('useImageZoom touch gestures', () => {
  it('pinches around the moving midpoint and reports user intent', () => {
    const onViewModeChange = vi.fn();
    const { result } = setup(onViewModeChange);

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
    });

    const start = touchEvent([
      { x: 310, y: 310 },
      { x: 510, y: 310 },
    ]);
    const move = touchEvent([
      { x: 260, y: 330 },
      { x: 660, y: 330 },
    ]);

    act(() => result.current.handleTouchStart(start.event));
    act(() => result.current.handleTouchMove(move.event));

    const state = result.current.zoomState;
    expect(state.scale).toBeCloseTo(0.8, 5);
    expect(state.offsetX).toBeCloseTo(-340, 5);
    expect(state.offsetY).toBeCloseTo(-203.2, 1);
    expect(start.preventDefault).toHaveBeenCalled();
    expect(move.preventDefault).toHaveBeenCalled();
    expect(onViewModeChange).toHaveBeenLastCalledWith('user');
  });

  it('pans a zoomed image with one finger', () => {
    const { result } = setup();

    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit');
      result.current.zoomIn();
    });
    const before = result.current.zoomState;
    const start = touchEvent([{ x: 400, y: 300 }]);
    const move = touchEvent([{ x: 300, y: 220 }]);

    act(() => result.current.handleTouchStart(start.event));
    act(() => result.current.handleTouchMove(move.event));

    expect(result.current.zoomState.offsetX).toBeCloseTo(
      before.offsetX - 100,
      5
    );
    expect(result.current.zoomState.offsetY).toBeCloseTo(
      before.offsetY - 80,
      5
    );
    expect(move.preventDefault).toHaveBeenCalled();
  });
});

describe('useImageZoom zoomTo100', () => {
  it('targets 1:1 of the ORIGINAL pixels when showing a downscaled preview', () => {
    const { result } = setup();

    act(() => {
      result.current.setImageDimensions(6000, 3999, true); // metadata
    });
    act(() => {
      result.current.applyBitmapDimensions(2000, 1333, 'fit'); // preview bitmap
    });

    act(() => {
      result.current.zoomTo100();
    });

    // scale 3.0 upscales the preview to the original's pixel grid — which is
    // what flips the detail view over to the original artifact.
    expect(result.current.zoomState.scale).toBeCloseTo(3.0, 5);
  });
});
