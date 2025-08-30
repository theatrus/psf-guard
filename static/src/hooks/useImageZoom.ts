import { useState, useCallback, useRef, useEffect } from 'react';

interface ZoomState {
  scale: number;
  offsetX: number;
  offsetY: number;
}

interface ZoomBounds {
  minScale: number;
  maxScale: number;
}

export interface UseImageZoomReturn {
  zoomState: ZoomState;
  containerRef: React.RefObject<HTMLDivElement | null>;
  imageRef: React.RefObject<HTMLImageElement | null>;
  handleWheel: (e: React.WheelEvent) => void;
  handleMouseDown: (e: React.MouseEvent) => void;
  handleMouseMove: (e: React.MouseEvent) => void;
  handleMouseUp: (e: React.MouseEvent) => void;
  handleKeyDown: (e: React.KeyboardEvent) => void;
  zoomIn: () => void;
  zoomOut: () => void;
  zoomToFit: () => void;
  zoomTo100: () => void;
  resetZoom: () => void;
  getZoomPercentage: () => number;
}

const DEFAULT_BOUNDS: ZoomBounds = {
  minScale: 0.1,
  maxScale: 10.0,
};

const ZOOM_STEP = 0.1;
const KEYBOARD_ZOOM_STEP = 0.2;

export function useImageZoom(bounds: ZoomBounds = DEFAULT_BOUNDS): UseImageZoomReturn {
  const [zoomState, setZoomState] = useState<ZoomState>({
    scale: 1,
    offsetX: 0,
    offsetY: 0,
  });
  
  // Flag to prevent auto-fit interference with intentional zoom operations
  const intentionalZoomRef = useRef(false);

  const containerRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const isPanningRef = useRef(false);
  const lastMousePosRef = useRef({ x: 0, y: 0 });
  const initialFitScaleRef = useRef(1);

  // Calculate the fit-to-screen scale when image loads
  const calculateFitScale = useCallback(() => {
    const container = containerRef.current;
    const image = imageRef.current;
    
    if (!container || !image || !image.naturalWidth || !image.naturalHeight) {
      return 1;
    }

    const containerRect = container.getBoundingClientRect();
    const containerWidth = containerRect.width - 20; // Minimal padding
    const containerHeight = containerRect.height - 20;
    
    const scaleX = containerWidth / image.naturalWidth;
    const scaleY = containerHeight / image.naturalHeight;
    
    // Use the smaller scale to ensure the image fits entirely, but allow scaling up for small images
    return Math.min(scaleX, scaleY);
  }, []);

  // Reset to fit-to-screen when image changes
  const zoomToFit = useCallback(() => {
    const container = containerRef.current;
    const image = imageRef.current;
    
    if (!container || !image || !image.naturalWidth || !image.naturalHeight) {
      setZoomState({
        scale: 1,
        offsetX: 0,
        offsetY: 0,
      });
      return;
    }

    const fitScale = calculateFitScale();
    initialFitScaleRef.current = fitScale;
    
    // Calculate the centered position for the image
    const containerRect = container.getBoundingClientRect();
    const containerWidth = containerRect.width;
    const containerHeight = containerRect.height;
    
    const scaledImageWidth = image.naturalWidth * fitScale;
    const scaledImageHeight = image.naturalHeight * fitScale;
    
    // Calculate offsets to center the image in the container
    const offsetX = (containerWidth - scaledImageWidth) / 2;
    const offsetY = (containerHeight - scaledImageHeight) / 2;
    
    setZoomState({
      scale: fitScale,
      offsetX: offsetX,
      offsetY: offsetY,
    });
  }, [calculateFitScale]);

  // Zoom to 100% (actual size)
  const zoomTo100 = useCallback(() => {
    // Mark this as an intentional zoom operation
    intentionalZoomRef.current = true;
    
    const container = containerRef.current;
    const image = imageRef.current;
    
    if (!container || !image || !image.naturalWidth || !image.naturalHeight) {
      setZoomState({
        scale: 1,
        offsetX: 0,
        offsetY: 0,
      });
      return;
    }

    // Calculate the centered position for 100% zoom
    const containerRect = container.getBoundingClientRect();
    const containerWidth = containerRect.width;
    const containerHeight = containerRect.height;
    
    const imageWidth = image.naturalWidth; // 1:1 scale
    const imageHeight = image.naturalHeight;
    
    // Always center at 100% - don't apply constraints here as we want actual 100% zoom
    const offsetX = (containerWidth - imageWidth) / 2;
    const offsetY = (containerHeight - imageHeight) / 2;
    
    setZoomState({
      scale: 1,
      offsetX: offsetX,
      offsetY: offsetY,
    });
  }, []);

  // Reset zoom (alias for zoomToFit)
  const resetZoom = useCallback(() => {
    zoomToFit();
  }, [zoomToFit]);

  // Clamp zoom scale within bounds
  const clampScale = useCallback((scale: number): number => {
    return Math.max(bounds.minScale, Math.min(bounds.maxScale, scale));
  }, [bounds]);

  // Constrain pan offsets to keep image mostly visible
  const constrainPan = useCallback((offsetX: number, offsetY: number, scale: number): { offsetX: number; offsetY: number } => {
    const container = containerRef.current;
    const image = imageRef.current;
    
    if (!container || !image || !image.naturalWidth || !image.naturalHeight) {
      return { offsetX, offsetY };
    }

    const containerRect = container.getBoundingClientRect();
    const containerWidth = containerRect.width;
    const containerHeight = containerRect.height;
    
    const scaledImageWidth = image.naturalWidth * scale;
    const scaledImageHeight = image.naturalHeight * scale;
    
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

  // Handle mouse wheel zoom
  const handleWheel = useCallback((e: React.WheelEvent) => {
    e.preventDefault();
    
    const container = containerRef.current;
    if (!container) return;

    const rect = container.getBoundingClientRect();
    const mouseX = e.clientX - rect.left;
    const mouseY = e.clientY - rect.top;

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
  }, [clampScale]);

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
  const handleMouseUp = useCallback((_e: React.MouseEvent) => {
    isPanningRef.current = false;
  }, []);

  // Zoom in function
  const zoomIn = useCallback(() => {
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

  // Get zoom percentage
  const getZoomPercentage = useCallback((): number => {
    return Math.round(zoomState.scale * 100);
  }, [zoomState.scale]);

  // Initialize fit scale when component mounts or image changes
  useEffect(() => {
    const container = containerRef.current;
    const image = imageRef.current;
    if (container && image && image.complete && image.naturalWidth > 0) {
      const fitScale = calculateFitScale();
      initialFitScaleRef.current = fitScale;
      if (zoomState.scale === 1 && !intentionalZoomRef.current) {
        // Only auto-fit if we're at the default scale and it's not intentional
        setZoomState({
          scale: fitScale,
          offsetX: 0,
          offsetY: 0,
        });
      }
      // Reset the flag after any potential auto-fit
      intentionalZoomRef.current = false;
    }
  }, [calculateFitScale, zoomState.scale]);

  return {
    zoomState,
    containerRef,
    imageRef,
    handleWheel,
    handleMouseDown,
    handleMouseMove,
    handleMouseUp,
    handleKeyDown,
    zoomIn,
    zoomOut,
    zoomToFit,
    zoomTo100,
    resetZoom,
    getZoomPercentage,
  };
}