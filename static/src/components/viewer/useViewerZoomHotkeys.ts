import { useHotkeys } from 'react-hotkeys-hook';

interface ViewerZoom {
  zoomIn: () => void;
  zoomOut: () => void;
  zoomToFit: () => void;
  zoomTo100: () => void;
}

/** The zoom keys both image viewers take: + and −, 0 or F to fit, 1 for 100%. */
export function useViewerZoomHotkeys(zoom: ViewerZoom) {
  useHotkeys('plus,equal', () => zoom.zoomIn(), [zoom.zoomIn]);
  useHotkeys('minus', () => zoom.zoomOut(), [zoom.zoomOut]);
  useHotkeys('0,f', () => zoom.zoomToFit(), [zoom.zoomToFit]);
  useHotkeys('1', () => zoom.zoomTo100(), [zoom.zoomTo100]);
}
