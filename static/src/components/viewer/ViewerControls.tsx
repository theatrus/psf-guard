import type { ReactNode } from 'react';
import { toggleMeasureOption, type MeasureOptions } from '../../hooks/useMeasureOptions';
import './viewer.css';

interface ZoomControlsProps {
  percentage: number;
  onZoomOut: () => void;
  onFit: () => void;
  onActualSize: () => void;
  onZoomIn: () => void;
  className?: string;
  /** More toggles in the same row, such as the reviewer's Color. */
  children?: ReactNode;
}

/** The zoom percentage and buttons both image viewers show. */
export function ZoomControls({
  percentage,
  onZoomOut,
  onFit,
  onActualSize,
  onZoomIn,
  className = '',
  children,
}: ZoomControlsProps) {
  return (
    <>
      <div className="zoom-info-compact">
        <span className="zoom-percentage-compact">{percentage}%</span>
      </div>
      <div className={`zoom-buttons-compact ${className}`.trim()}>
        <button className="zoom-btn-compact" type="button" onClick={onZoomOut} title="Zoom Out (-)">−</button>
        <button className="zoom-btn-compact" type="button" onClick={onFit} title="Fit to Screen (F)">Fit</button>
        <button className="zoom-btn-compact" type="button" onClick={onActualSize} title="100% (1)">100%</button>
        <button className="zoom-btn-compact" type="button" onClick={onZoomIn} title="Zoom In (+)">+</button>
        {children}
      </div>
    </>
  );
}

interface MeasureTogglesProps {
  options: MeasureOptions;
  /** Why the sky position cannot be shown, which disables its toggle. */
  skyDisabled?: string;
  /** Why values cannot be read, which disables their toggle. */
  valueDisabled?: string;
}

/** Turn the pointer readout's value and RA/Dec on and off (M and R). */
export function MeasureToggles({ options, skyDisabled, valueDisabled }: MeasureTogglesProps) {
  return (
    <div className="measure-toggles" role="group" aria-label="Pointer readout">
      <button
        className="zoom-btn-compact"
        type="button"
        aria-pressed={options.value}
        disabled={!!valueDisabled}
        onClick={() => toggleMeasureOption('value')}
        title={valueDisabled ?? 'Show the linear pixel value under the pointer (M)'}
      >
        Value
      </button>
      <button
        className="zoom-btn-compact"
        type="button"
        aria-pressed={options.sky}
        disabled={!!skyDisabled}
        onClick={() => toggleMeasureOption('sky')}
        title={skyDisabled ?? 'Show the right ascension and declination under the pointer (R)'}
      >
        RA/Dec
      </button>
    </div>
  );
}
