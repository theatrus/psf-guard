import { Fragment, useEffect, useRef } from 'react';
import { apiClient } from '../../api/client';
import type { BrowsedCalibrationFrame, CalibrationFrameStretch, FlatClockRun } from '../../api/types';
import { useAsyncImage } from '../../hooks/useAsyncImage';
import { fileName, formatTime, gradeLabel, suspectOverruled } from './calibrationLabels';
import { useRigClocks } from './useRigClocks';

interface ThumbProps {
  dbId: string;
  frame: BrowsedCalibrationFrame;
  index: number;
  stretch: CalibrationFrameStretch;
  selected: boolean;
  onSelect: (uuid: string) => void;
  clock?: FlatClockRun[];
}

function Thumb({ dbId, frame, index, stretch, selected, onSelect, clock }: ThumbProps) {
  const options = { view: 'frame', size: 'thumb', stretch } as const;
  const image = useAsyncImage(
    dbId,
    apiClient.getCalibrationFramePreviewUrl(dbId, frame.frame_uuid, options),
    { kind: 'calibration_frame', frameUuid: frame.frame_uuid, ...options }
  );
  const overruled = suspectOverruled(frame);
  const grade = overruled ? 'Used anyway' : gradeLabel(frame);
  const state = frame.kept_out ? 'kept-out' : overruled ? 'ok' : frame.grade ?? 'ungraded';
  return (
    <li>
      <button
        type="button"
        className={`calibration-thumb calibration-thumb-${state}`}
        aria-pressed={selected}
        aria-label={`Frame ${index + 1}: ${fileName(frame.source_path)}`}
        title={`${fileName(frame.source_path)}\n${formatTime(frame.captured_at, clock)}${grade ? `\n${grade}` : ''}${frame.kept_out ? `\nKept out: ${frame.kept_out}` : ''}`}
        data-frame-uuid={frame.frame_uuid}
        onClick={() => onSelect(frame.frame_uuid)}
      >
        <span className="calibration-thumb-image">
          {frame.source_exists === false ? (
            <span className="calibration-thumb-note">Missing</span>
          ) : image.state === 'error' ? (
            <span className="calibration-thumb-note">No picture</span>
          ) : (
            <>
              {image.state !== 'ready' && <span className="calibration-thumb-note">…</span>}
              <img
                src={image.src}
                alt=""
                loading="lazy"
                draggable={false}
                onLoad={image.onLoad}
                onError={image.onError}
                style={{ visibility: image.state === 'ready' ? 'visible' : 'hidden' }}
              />
            </>
          )}
        </span>
        <span className="calibration-thumb-label">
          <span>{index + 1}</span>
          {frame.kept_out ? <span className="calibration-badge calibration-badge-poor">Out</span>
            : frame.grade === 'poor' ? <span className="calibration-badge calibration-badge-poor">Poor</span>
              : frame.grade === 'suspect' && !overruled ? <span className="calibration-badge calibration-badge-suspect">Check</span>
                : null}
        </span>
      </button>
    </li>
  );
}

/** A run of frames the strip marks off, such as one night of a flat group. */
export interface FrameRun {
  key: string;
  /** Its name, first: `2026-10-03`. */
  label: string;
  /** More about it: `10 flats`. */
  detail?: string;
  /** The colour of the group it belongs to, 1 to 4. */
  tone?: number;
}

interface CalibrationFrameStripProps {
  dbId: string;
  frames: BrowsedCalibrationFrame[];
  selected: string | null;
  stretch: CalibrationFrameStretch;
  onSelect: (uuid: string) => void;
  /** Mark where each run begins, with its name. */
  runOf?: (frame: BrowsedCalibrationFrame) => FrameRun;
}

/** Frames in order as small pictures: one session's, or a flat group's
 *  night by night, each night marked where it begins. */
export default function CalibrationFrameStrip({ dbId, frames, selected, stretch, onSelect, runOf }: CalibrationFrameStripProps) {
  const clockOf = useRigClocks(dbId);
  const listRef = useRef<HTMLOListElement>(null);
  // Keep the chosen frame in sight as the arrow keys move through the
  // strip. Only the strip scrolls; the page stays where the reader left it.
  useEffect(() => {
    const list = listRef.current;
    const button = selected ? list?.querySelector<HTMLElement>(`[data-frame-uuid="${selected}"]`) : null;
    if (!list || !button) return;
    const left = button.offsetLeft;
    const right = left + button.offsetWidth;
    if (left < list.scrollLeft) list.scrollLeft = left;
    else if (right > list.scrollLeft + list.clientWidth) list.scrollLeft = right - list.clientWidth;
  }, [selected, frames.length]);
  return (
    <ol className="calibration-strip" ref={listRef} aria-label="Frames">
      {frames.map((frame, index) => {
        const run = runOf?.(frame);
        const starts = run && (index === 0 || runOf?.(frames[index - 1]).key !== run.key);
        return (
          <Fragment key={frame.frame_uuid}>
            {starts && (
              <li className="calibration-strip-run" data-flat-tone={run.tone} data-run={run.key}
                data-testid="calibration-strip-run">
                <span className="calibration-strip-run-label">
                  <strong>{run.label}</strong>
                  {run.detail && <span>{run.detail}</span>}
                </span>
              </li>
            )}
            <Thumb
              dbId={dbId}
              clock={clockOf(frame.rig_uuid)}
              frame={frame}
              index={index}
              stretch={stretch}
              selected={frame.frame_uuid === selected}
              onSelect={onSelect}
            />
          </Fragment>
        );
      })}
    </ol>
  );
}
