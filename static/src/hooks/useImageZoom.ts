import { useState, useCallback, useRef, useEffect } from 'react';

interface ZoomState {
  scale: number;
  offsetX: number;
  offsetY: number;
  visualScale?: number; // Visual scale relative to original image size
}

interface ZoomBounds {
  minScale: number;
  maxScale: number;
}

export type ZoomViewMode = 'fit' | 'user';

export interface UseImageZoomOptions extends ZoomBounds {
  /**
   * Notified when the view intent changes: 'user' after any explicit zoom or
   * pan, 'fit' after an explicit fit/reset. Programmatic dimension updates
   * (applyBitmapDimensions / adjustZoomForNewImage) never fire this, so the
   * caller can use it to decide whether to preserve the view on image loads.
   */
  onViewModeChange?: (mode: ZoomViewMode) => void;
}

export interface UseImageZoomReturn {
  zoomState: ZoomState;
  containerRef: React.RefObject<HTMLDivElement | null>;
  imageRef: React.RefObject<HTMLImageElement | null>;
  handleWheel: (e: React.WheelEvent) => void;
  handleMouseDown: (e: React.MouseEvent) => void;
  handleMouseMove: (e: React.MouseEvent) => void;
  handleMouseUp: (e: React.MouseEvent) => void;
  handleTouchStart: (e: React.TouchEvent) => void;
  handleTouchMove: (e: React.TouchEvent) => void;
  handleTouchEnd: (e: React.TouchEvent) => void;
  handleKeyDown: (e: React.KeyboardEvent) => void;
  zoomIn: () => void;
  zoomOut: () => void;
  zoomToFit: () => void;
  zoomToFitDimensions: (width: number, height: number) => void;
  zoomTo100: () => void;
  resetZoom: () => void;
  getZoomPercentage: () => number;
  resetInitialization: () => void;
  setZoomState: React.Dispatch<React.SetStateAction<ZoomState>>;
  adjustZoomForNewImage: (oldWidth: number, oldHeight: number, newWidth: number, newHeight: number) => void;
  applyBitmapDimensions: (width: number, height: number, mode: 'fit' | 'preserve') => void;
  notifyBitmapDimensions: (width: number, height: number) => void;
  setImageDimensions: (width: number, height: number, isOriginal: boolean) => void;
  getVisualScale: () => number;
  hasOverflow: boolean;
}

const DEFAULT_BOUNDS: ZoomBounds = {
  minScale: 0.1,
  maxScale: 10.0,
};

const ZOOM_STEP = 0.1;
const KEYBOARD_ZOOM_STEP = 0.2;

export function useImageZoom(options: UseImageZoomOptions = DEFAULT_BOUNDS): UseImageZoomReturn {
  const bounds: ZoomBounds = options;
  const [zoomState, setZoomState] = useState<ZoomState>({
    scale: 1,
    offsetX: 0,
    offsetY: 0,
    visualScale: 1,
  });

  // Flag to prevent auto-fit interference with intentional zoom operations
  const intentionalZoomRef = useRef(false);
  const hasInitializedRef = useRef(false);

  const containerRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const isPanningRef = useRef(false);
  const lastMousePosRef = useRef({ x: 0, y: 0 });
  const lastTouchPosRef = useRef<{ x: number; y: number } | null>(null);
  const pinchRef = useRef<{
    distance: number;
    centerX: number;
    centerY: number;
    state: ZoomState;
  } | null>(null);
  const initialFitScaleRef = useRef(1);

  // Latest onViewModeChange without forcing callers to memoize it.
  const onViewModeChangeRef = useRef(options.onViewModeChange);
  onViewModeChangeRef.current = options.onViewModeChange;

  // Dimensions of the bitmap the CURRENT zoom state (scale/offsets) is
  // calibrated against. This is the source of truth for pan constraints and
  // fits — never the live <img>.naturalWidth, which lags the state during a
  // src swap (large → original) and used to clamp original-image offsets
  // against the old preview's dimensions, throwing the viewport to the
  // top-left corner mid-gesture.
  const stateDimsRef = useRef<{ width: number; height: number } | null>(null);

  // Track original image dimensions
  const originalDimensionsRef = useRef<{ width: number; height: number } | null>(null);
  const currentImageIsOriginalRef = useRef(false);

  const calculateFitScaleForDimensions = useCallback((width: number, height: number) => {
    const container = containerRef.current;

    if (!container || !width || !height) {
      return 1;
    }

    const containerRect = container.getBoundingClientRect();
    const containerWidth = containerRect.width - 20; // Minimal padding
    const containerHeight = containerRect.height - 20;

    const scaleX = containerWidth / width;
    const scaleY = containerHeight / height;
    
    // Use the smaller scale to ensure the image fits entirely, but allow scaling up for small images
    return Math.min(scaleX, scaleY);
  }, []);

  // Calculate the fit-to-screen scale when image loads
  const calculateFitScale = useCallback(() => {
    const container = containerRef.current;
    const image = imageRef.current;

    if (!container || !image || !image.naturalWidth || !image.naturalHeight) {
      return 1;
    }

    return calculateFitScaleForDimensions(image.naturalWidth, image.naturalHeight);
  }, [calculateFitScaleForDimensions]);

  const zoomToFitDimensions = useCallback((width: number, height: number) => {
    const container = containerRef.current;

    if (!container || !width || !height) {
      setZoomState({
        scale: 1,
        offsetX: 0,
        offsetY: 0,
        visualScale: 1,
      });
      return;
    }

    const fitScale = calculateFitScaleForDimensions(width, height);
    initialFitScaleRef.current = fitScale;
    stateDimsRef.current = { width, height };

    // Calculate the centered position for the image
    const containerRect = container.getBoundingClientRect();
    const containerWidth = containerRect.width;
    const containerHeight = containerRect.height;

    const scaledImageWidth = width * fitScale;
    const scaledImageHeight = height * fitScale;

    // Calculate offsets to center the image in the container
    const offsetX = (containerWidth - scaledImageWidth) / 2;
    const offsetY = (containerHeight - scaledImageHeight) / 2;

    // Calculate visual scale based on original dimensions if available
    let visualScale = fitScale;
    if (originalDimensionsRef.current && !currentImageIsOriginalRef.current) {
      const ratio = width / originalDimensionsRef.current.width;
      visualScale = fitScale * ratio;
    }

    setZoomState({
      scale: fitScale,
      offsetX: offsetX,
      offsetY: offsetY,
      visualScale: visualScale,
    });
    
    // Mark as initialized when we manually fit
    hasInitializedRef.current = true;
  }, [calculateFitScaleForDimensions]);

  // Reset to fit-to-screen when image changes
  const zoomToFit = useCallback(() => {
    onViewModeChangeRef.current?.('fit');

    const image = imageRef.current;
    const width = image?.naturalWidth || stateDimsRef.current?.width || 0;
    const height = image?.naturalHeight || stateDimsRef.current?.height || 0;

    // No dimensions known yet — keep the current state rather than hard
    // resetting to a top-left scale-1 view; the 'fit' intent above makes the
    // next load fit instead.
    if (!width || !height) return;

    zoomToFitDimensions(width, height);
  }, [zoomToFitDimensions]);

  // Reset zoom (alias for zoomToFit)
  const resetZoom = useCallback(() => {
    zoomToFit();
  }, [zoomToFit]);

  // Clamp zoom scale within bounds
  const clampScale = useCallback((scale: number): number => {
    return Math.max(bounds.minScale, Math.min(bounds.maxScale, scale));
  }, [bounds]);

  // Constrain pan offsets to keep image mostly visible
  const constrainPan = useCallback((
    offsetX: number,
    offsetY: number,
    scale: number,
    imageWidth?: number,
    imageHeight?: number
  ): { offsetX: number; offsetY: number } => {
    const container = containerRef.current;
    const image = imageRef.current;

    // Prefer the dimensions the zoom state is calibrated against over the
    // live <img> — during a bitmap swap the element still reports the OLD
    // image's size and would clamp the new state's offsets wrongly.
    const naturalWidth =
      imageWidth ?? stateDimsRef.current?.width ?? image?.naturalWidth;
    const naturalHeight =
      imageHeight ?? stateDimsRef.current?.height ?? image?.naturalHeight;

    if (!container || !naturalWidth || !naturalHeight) {
      return { offsetX, offsetY };
    }

    const containerRect = container.getBoundingClientRect();
    const containerWidth = containerRect.width;
    const containerHeight = containerRect.height;
    
    const scaledImageWidth = naturalWidth * scale;
    const scaledImageHeight = naturalHeight * scale;
    
    // Allow a small margin (10% of image size) to pan slightly past edges
    const marginX = scaledImageWidth * 0.1;
    const marginY = scaledImageHeight * 0.1;
    
    // Calculate bounds - image can be moved but not completely off screen
    const minX = containerWidth - scaledImageWidth - marginX;
    const maxX = marginX;
    const minY = containerHeight - scaledImageHeight - marginY;
    const maxY = marginY;
    
    // If image is smaller than container, center it
    if (scaledImageWidth <= containerWidth) {
      offsetX = (containerWidth - scaledImageWidth) / 2;
    } else {
      offsetX = Math.max(minX, Math.min(maxX, offsetX));
    }
    
    if (scaledImageHeight <= containerHeight) {
      offsetY = (containerHeight - scaledImageHeight) / 2;
    } else {
      offsetY = Math.max(minY, Math.min(maxY, offsetY));
    }
    
    return { offsetX, offsetY };
  }, []);

  // Zoom to 100% (actual size)
  const zoomTo100 = useCallback(() => {
    // Mark this as an intentional zoom operation
    intentionalZoomRef.current = true;
    onViewModeChangeRef.current?.('user');

    const container = containerRef.current;
    const image = imageRef.current;
    const bitmapWidth =
      stateDimsRef.current?.width || image?.naturalWidth || 0;

    if (!container || !bitmapWidth) {
      setZoomState({
        scale: 1,
        offsetX: 0,
        offsetY: 0,
      });
      return;
    }

    // 100% means one ORIGINAL image pixel per screen pixel. When the current
    // bitmap is a downscaled preview this lands at scale > 1 — exactly what
    // triggers the swap to the original-resolution artifact.
    const targetScale = clampScale(
      originalDimensionsRef.current
        ? originalDimensionsRef.current.width / bitmapWidth
        : 1
    );

    const containerRect = container.getBoundingClientRect();
    const viewportCenterX = containerRect.width / 2;
    const viewportCenterY = containerRect.height / 2;

    setZoomState(prevState => {
      // Keep the image point currently at the viewport center fixed.
      const imageX = (viewportCenterX - prevState.offsetX) / prevState.scale;
      const imageY = (viewportCenterY - prevState.offsetY) / prevState.scale;
      const constrained = constrainPan(
        viewportCenterX - imageX * targetScale,
        viewportCenterY - imageY * targetScale,
        targetScale
      );
      return {
        scale: targetScale,
        offsetX: constrained.offsetX,
        offsetY: constrained.offsetY,
      };
    });
  }, [clampScale, constrainPan]);

  // Handle mouse wheel zoom
  const handleWheel = useCallback((e: React.WheelEvent) => {
    e.preventDefault();

    const container = containerRef.current;
    if (!container) return;

    const rect = container.getBoundingClientRect();
    const mouseX = e.clientX - rect.left;
    const mouseY = e.clientY - rect.top;

    // Mark this as an intentional zoom operation
    intentionalZoomRef.current = true;
    onViewModeChangeRef.current?.('user');

    setZoomState(prevState => {
      const delta = -e.deltaY * 0.01;
      const newScale = clampScale(prevState.scale + delta * ZOOM_STEP);
      
      if (newScale === prevState.scale) return prevState;

      // Zoom toward mouse cursor
      const scaleFactor = newScale / prevState.scale;
      const newOffsetX = mouseX - scaleFactor * (mouseX - prevState.offsetX);
      const newOffsetY = mouseY - scaleFactor * (mouseY - prevState.offsetY);
      
      // Apply constraints to prevent image from going off-screen
      const constrained = constrainPan(newOffsetX, newOffsetY, newScale);

      return {
        scale: newScale,
        offsetX: constrained.offsetX,
        offsetY: constrained.offsetY,
      };
    });
  }, [clampScale, constrainPan]);

  // Handle mouse down for panning
  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button !== 0) return; // Only left mouse button
    
    isPanningRef.current = true;
    lastMousePosRef.current = { x: e.clientX, y: e.clientY };
    
    // Prevent image drag
    e.preventDefault();
  }, []);

  // Handle mouse move for panning
  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    if (!isPanningRef.current) return;

    const deltaX = e.clientX - lastMousePosRef.current.x;
    const deltaY = e.clientY - lastMousePosRef.current.y;

    if (deltaX !== 0 || deltaY !== 0) {
      onViewModeChangeRef.current?.('user');
    }

    setZoomState(prevState => {
      const newOffsetX = prevState.offsetX + deltaX;
      const newOffsetY = prevState.offsetY + deltaY;
      const constrained = constrainPan(newOffsetX, newOffsetY, prevState.scale);
      
      return {
        ...prevState,
        offsetX: constrained.offsetX,
        offsetY: constrained.offsetY,
      };
    });

    lastMousePosRef.current = { x: e.clientX, y: e.clientY };
  }, [constrainPan]);

  // Handle mouse up for panning
  const handleMouseUp = useCallback(() => {
    isPanningRef.current = false;
  }, []);

  const beginPinch = useCallback((touches: React.TouchList, state: ZoomState) => {
    const container = containerRef.current;
    if (!container || touches.length < 2) return;

    const rect = container.getBoundingClientRect();
    const first = touches[0];
    const second = touches[1];
    const deltaX = second.clientX - first.clientX;
    const deltaY = second.clientY - first.clientY;

    pinchRef.current = {
      distance: Math.hypot(deltaX, deltaY),
      centerX: (first.clientX + second.clientX) / 2 - rect.left,
      centerY: (first.clientY + second.clientY) / 2 - rect.top,
      state,
    };
    lastTouchPosRef.current = null;
  }, []);

  // Touch uses one finger to pan and two fingers to zoom around their
  // midpoint. The paired touch-action rule on .zoom-container keeps the
  // browser from taking over the gesture.
  const handleTouchStart = useCallback((e: React.TouchEvent) => {
    if (e.touches.length >= 2) {
      e.preventDefault();
      beginPinch(e.touches, zoomState);
      return;
    }

    if (e.touches.length === 1) {
      const touch = e.touches[0];
      lastTouchPosRef.current = { x: touch.clientX, y: touch.clientY };
      pinchRef.current = null;
    }
  }, [beginPinch, zoomState]);

  const handleTouchMove = useCallback((e: React.TouchEvent) => {
    const container = containerRef.current;
    if (!container) return;

    if (e.touches.length >= 2) {
      e.preventDefault();

      if (!pinchRef.current) {
        beginPinch(e.touches, zoomState);
        return;
      }

      const first = e.touches[0];
      const second = e.touches[1];
      const rect = container.getBoundingClientRect();
      const deltaX = second.clientX - first.clientX;
      const deltaY = second.clientY - first.clientY;
      const distance = Math.hypot(deltaX, deltaY);
      const centerX = (first.clientX + second.clientX) / 2 - rect.left;
      const centerY = (first.clientY + second.clientY) / 2 - rect.top;
      const pinch = pinchRef.current;

      if (pinch.distance <= 0) return;

      const newScale = clampScale(
        pinch.state.scale * (distance / pinch.distance)
      );
      const imageX =
        (pinch.centerX - pinch.state.offsetX) / pinch.state.scale;
      const imageY =
        (pinch.centerY - pinch.state.offsetY) / pinch.state.scale;
      const constrained = constrainPan(
        centerX - imageX * newScale,
        centerY - imageY * newScale,
        newScale
      );

      intentionalZoomRef.current = true;
      onViewModeChangeRef.current?.('user');
      setZoomState({
        scale: newScale,
        offsetX: constrained.offsetX,
        offsetY: constrained.offsetY,
      });
      return;
    }

    if (e.touches.length === 1 && lastTouchPosRef.current) {
      e.preventDefault();
      const touch = e.touches[0];
      const deltaX = touch.clientX - lastTouchPosRef.current.x;
      const deltaY = touch.clientY - lastTouchPosRef.current.y;

      if (deltaX !== 0 || deltaY !== 0) {
        onViewModeChangeRef.current?.('user');
        setZoomState(prevState => {
          const constrained = constrainPan(
            prevState.offsetX + deltaX,
            prevState.offsetY + deltaY,
            prevState.scale
          );
          return {
            ...prevState,
            offsetX: constrained.offsetX,
            offsetY: constrained.offsetY,
          };
        });
      }

      lastTouchPosRef.current = { x: touch.clientX, y: touch.clientY };
    }
  }, [beginPinch, clampScale, constrainPan, zoomState]);

  const handleTouchEnd = useCallback((e: React.TouchEvent) => {
    if (e.touches.length >= 2) {
      beginPinch(e.touches, zoomState);
    } else if (e.touches.length === 1) {
      const touch = e.touches[0];
      pinchRef.current = null;
      lastTouchPosRef.current = { x: touch.clientX, y: touch.clientY };
    } else {
      pinchRef.current = null;
      lastTouchPosRef.current = null;
    }
  }, [beginPinch, zoomState]);

  // Zoom in function
  const zoomIn = useCallback(() => {
    // Mark this as an intentional zoom operation
    intentionalZoomRef.current = true;
    onViewModeChangeRef.current?.('user');

    setZoomState(prevState => {
      const newScale = clampScale(prevState.scale + KEYBOARD_ZOOM_STEP);
      const constrained = constrainPan(prevState.offsetX, prevState.offsetY, newScale);
      return {
        scale: newScale,
        offsetX: constrained.offsetX,
        offsetY: constrained.offsetY,
      };
    });
  }, [clampScale, constrainPan]);

  // Zoom out function
  const zoomOut = useCallback(() => {
    // Mark this as an intentional zoom operation
    intentionalZoomRef.current = true;
    onViewModeChangeRef.current?.('user');

    setZoomState(prevState => {
      const newScale = clampScale(prevState.scale - KEYBOARD_ZOOM_STEP);
      const constrained = constrainPan(prevState.offsetX, prevState.offsetY, newScale);
      return {
        scale: newScale,
        offsetX: constrained.offsetX,
        offsetY: constrained.offsetY,
      };
    });
  }, [clampScale, constrainPan]);

  // Handle keyboard shortcuts
  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    switch (e.key) {
      case '+':
      case '=':
        e.preventDefault();
        zoomIn();
        break;
      case '-':
      case '_':
        e.preventDefault();
        zoomOut();
        break;
      case '0':
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          resetZoom();
        }
        break;
      case '1':
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          zoomTo100();
        }
        break;
    }
  }, [zoomIn, zoomOut, resetZoom, zoomTo100]);

  // Get zoom percentage relative to the ORIGINAL image's pixels: 100% = one
  // original pixel per screen pixel, whichever bitmap (preview or original)
  // is currently displayed. This keeps the number stable across bitmap swaps
  // and across navigation.
  const getZoomPercentage = useCallback((): number => {
    const dims = stateDimsRef.current;
    const original = originalDimensionsRef.current;
    const ratio = dims && original ? dims.width / original.width : 1;
    return Math.round(zoomState.scale * ratio * 100);
  }, [zoomState.scale]);
  
  // Get visual scale
  const getVisualScale = useCallback((): number => {
    return zoomState.visualScale || zoomState.scale;
  }, [zoomState.scale, zoomState.visualScale]);
  
  // Reset initialization state (for when image changes)
  const resetInitialization = useCallback(() => {
    hasInitializedRef.current = false;
    intentionalZoomRef.current = false;
  }, []);
  
  // Set image dimensions and update visual scale
  const setImageDimensions = useCallback((width: number, height: number, isOriginal: boolean) => {
    const isFirstTime = !originalDimensionsRef.current;

    if (isOriginal) {
      const prev = originalDimensionsRef.current;
      // Update when meaningfully different so navigating to a target shot on
      // different gear doesn't keep a stale original size forever.
      if (
        !prev ||
        Math.abs(prev.width - width) > 10 ||
        Math.abs(prev.height - height) > 10
      ) {
        originalDimensionsRef.current = { width, height };
      }
    }

    // Update which image we're currently viewing
    currentImageIsOriginalRef.current = isOriginal;
    
    // Only update visual scale if this is the first time we're learning the original dimensions
    if (isFirstTime && originalDimensionsRef.current) {
      setZoomState(prevState => {
        const sizeRatio = isOriginal ? 1 : width / originalDimensionsRef.current!.width;
        const visualScale = prevState.scale * sizeRatio;
        return {
          ...prevState,
          visualScale,
        };
      });
    }
  }, []);
  
  // Adjust zoom when switching between different image sizes
  const adjustZoomForNewImage = useCallback((oldWidth: number, oldHeight: number, newWidth: number, newHeight: number) => {
    if (oldWidth === 0 || oldHeight === 0 || newWidth === 0 || newHeight === 0) return;

    const container = containerRef.current;
    if (!container) return;

    // Determine which image we're NOW viewing (not updated yet)
    const isNowOriginal = originalDimensionsRef.current
      ? Math.abs(newWidth - originalDimensionsRef.current.width) < 10
      : currentImageIsOriginalRef.current;

    // The state below is calibrated against the NEW bitmap from here on.
    stateDimsRef.current = { width: newWidth, height: newHeight };

    setZoomState(prevState => {
      // The key insight: we need to maintain the same DISPLAYED pixel size
      // If we were showing a 2000px image at scale 3.0 (displaying 6000px)
      // When we switch to a 6000px image, we need scale 1.0 (still displaying 6000px)
      
      // Calculate the displayed size with the current image and scale
      const currentDisplayedWidth = oldWidth * prevState.scale;
      
      // Calculate what scale we need for the new image to maintain the same displayed size
      const newScale = currentDisplayedWidth / newWidth;
      
      // The visual scale should remain unchanged
      const targetVisualScale = prevState.visualScale || prevState.scale;
      
      // Get current view center in viewport coordinates
      const containerRect = container.getBoundingClientRect();
      const viewportCenterX = containerRect.width / 2;
      const viewportCenterY = containerRect.height / 2;
      
      // Calculate what point in the old image was at the center
      const oldImageX = (viewportCenterX - prevState.offsetX) / prevState.scale;
      const oldImageY = (viewportCenterY - prevState.offsetY) / prevState.scale;
      
      // Scale the image coordinates by the size change ratio
      const widthChangeRatio = newWidth / oldWidth;
      const heightChangeRatio = newHeight / oldHeight;
      const newImageX = oldImageX * widthChangeRatio;
      const newImageY = oldImageY * heightChangeRatio;
      
      // Calculate new offsets to keep the same point at the center
      const newOffsetX = viewportCenterX - (newImageX * newScale);
      const newOffsetY = viewportCenterY - (newImageY * newScale);
      
      const constrained = constrainPan(newOffsetX, newOffsetY, newScale, newWidth, newHeight);
      
      return {
        scale: newScale,
        offsetX: constrained.offsetX,
        offsetY: constrained.offsetY,
        visualScale: targetVisualScale, // Keep visual scale unchanged
      };
    });
    
    // Update which image we're currently viewing AFTER the state update
    currentImageIsOriginalRef.current = isNowOriginal;
  }, [constrainPan]);

  // Report the dimensions of a bitmap that just finished loading into the
  // <img>. In 'fit' mode the view refits (centered); in 'preserve' mode the
  // view is kept EXACTLY when the dimensions match the state's calibration
  // (the arrow-key navigation case) and mapped to the same visual size and
  // center when they differ (preview ↔ original swaps, cross-size loads).
  const applyBitmapDimensions = useCallback(
    (width: number, height: number, mode: 'fit' | 'preserve') => {
      if (!width || !height) return;

      const prev = stateDimsRef.current;
      if (mode === 'fit' || !prev) {
        zoomToFitDimensions(width, height);
        return;
      }

      const changed =
        Math.abs(width - prev.width) > 10 || Math.abs(height - prev.height) > 10;
      if (changed) {
        adjustZoomForNewImage(prev.width, prev.height, width, height);
      } else {
        stateDimsRef.current = { width, height };
      }
    },
    [zoomToFitDimensions, adjustZoomForNewImage]
  );

  // The <img> now renders a bitmap of these dimensions — recalibrate pan
  // constraints and fit fallbacks against it WITHOUT touching the transform.
  // For callers that keep their own zoom-preservation logic (comparison view)
  // and don't route loads through applyBitmapDimensions: since constrainPan
  // prefers stateDimsRef over the live <img>, every loaded bitmap must be
  // reported through one of these paths or constraints clamp against the
  // previous image's dimensions.
  const notifyBitmapDimensions = useCallback((width: number, height: number) => {
    if (!width || !height) return;
    stateDimsRef.current = { width, height };
  }, []);

  // Initialize fit scale when component mounts or image changes
  useEffect(() => {
    const container = containerRef.current;
    const image = imageRef.current;
    if (container && image && image.complete && image.naturalWidth > 0) {
      const fitScale = calculateFitScale();
      initialFitScaleRef.current = fitScale;

      // Only auto-fit if we haven't initialized yet
      if (!hasInitializedRef.current && zoomState.scale === 1 && !intentionalZoomRef.current) {
        zoomToFitDimensions(image.naturalWidth, image.naturalHeight);
      }
      // Reset the intentional flag after any potential auto-fit
      intentionalZoomRef.current = false;
    }
  }, [calculateFitScale, zoomToFitDimensions, zoomState.scale]);

  // Calculate whether the image overflows the container
  const hasOverflow = (() => {
    const container = containerRef.current;
    const image = imageRef.current;
    
    if (!container || !image || !image.naturalWidth || !image.naturalHeight) {
      return false;
    }
    
    const containerRect = container.getBoundingClientRect();
    const scaledImageWidth = image.naturalWidth * zoomState.scale;
    const scaledImageHeight = image.naturalHeight * zoomState.scale;
    
    // Check if either dimension exceeds the container
    return scaledImageWidth > containerRect.width || scaledImageHeight > containerRect.height;
  })();

  return {
    zoomState,
    containerRef,
    imageRef,
    handleWheel,
    handleMouseDown,
    handleMouseMove,
    handleMouseUp,
    handleTouchStart,
    handleTouchMove,
    handleTouchEnd,
    handleKeyDown,
    zoomIn,
    zoomOut,
    zoomToFit,
    zoomToFitDimensions,
    zoomTo100,
    resetZoom,
    getZoomPercentage,
    resetInitialization,
    setZoomState,
    adjustZoomForNewImage,
    applyBitmapDimensions,
    notifyBitmapDimensions,
    setImageDimensions,
    getVisualScale,
    hasOverflow,
  };
}
