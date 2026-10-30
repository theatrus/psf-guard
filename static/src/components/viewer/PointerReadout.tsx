import { formatDec, formatRa } from '@seiza/astro-overlay';
import type { MeasureOptions } from '../../hooks/useMeasureOptions';
import type { PixelProbe } from '../../hooks/usePixelProbe';
import { formatSample } from '../../utils/pixelWindow';
import { readoutPlacement, type ImagePixel, type PointerAnchor } from './imagePointer';
import './viewer.css';

interface PointerReadoutProps {
  options: MeasureOptions;
  pixel: ImagePixel | null;
  /** Where the pointer is in the viewer. */
  anchor: PointerAnchor | null;
  /** The samples under the pointer; null when this image has no readable file. */
  probe: PixelProbe | null;
  /** The pointer's sky position; null when the image has no plate solution. */
  sky: [number, number] | null;
  /** Why the sky position is missing, when it is. */
  skyUnavailable?: string;
  /** Plane names for a three-plane image. */
  planeLabels?: string[];
}

const RGB = ['R', 'G', 'B'];

function valueText(probe: PixelProbe | null, planeLabels: string[]): string {
  if (!probe) return 'No values for this image';
  switch (probe.state) {
    case 'ready':
      return probe.values.length === 1
        ? formatSample(probe.values[0])
        : probe.values
          .map((value, index) => `${planeLabels[index] ?? index + 1} ${formatSample(value)}`)
          .join('  ');
    case 'error':
      return probe.message;
    case 'loading':
      return 'Reading…';
    default:
      return '—';
  }
}

/**
 * The pixel under the pointer, with its linear value and sky position when
 * the viewer turned those on, in a box that follows the pointer.
 */
export default function PointerReadout({
  options,
  pixel,
  anchor,
  probe,
  sky,
  skyUnavailable = 'No plate solution',
  planeLabels = RGB,
}: PointerReadoutProps) {
  if ((!options.value && !options.sky) || !pixel || !anchor) return null;
  return (
    <div className="pointer-readout" aria-hidden="true" data-testid="pointer-readout" style={readoutPlacement(anchor)}>
      {options.value && (
        <span className="pointer-readout-value" data-testid="pointer-readout-value">
          {valueText(probe, planeLabels)}
        </span>
      )}
      {options.sky && (
        <span className="pointer-readout-sky" data-testid="pointer-readout-sky">
          {sky ? <><span>{formatRa(sky[0])}</span> <span>{formatDec(sky[1])}</span></> : skyUnavailable}
        </span>
      )}
      <span className="pointer-readout-pixel">
        x {pixel.x} · y {pixel.y}
      </span>
    </div>
  );
}
