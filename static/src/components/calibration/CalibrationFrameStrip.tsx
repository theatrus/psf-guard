import { useEffect, useRef } from 'react';
import { apiClient } from '../../api/client';
import type { BrowsedCalibrationFrame, CalibrationFrameStretch } from '../../api/types';
import { useAsyncImage } from '../../hooks/useAsyncImage';
import { fileName, formatTime, gradeLabel } from './calibrationLabels';

interface ThumbProps {
  dbId: string;
  frame: BrowsedCalibrationFrame;
  index: number;
  stretch: CalibrationFrameStretch;
  selected: boolean;
  onSelect: (uuid: string) => void;
}

function Thumb({ dbId, frame, index, stretch, selected, onSelect }: ThumbProps) {
  const options = { view: 'frame', size: 'thumb', stretch } as const;
  const image = useAsyncImage(
    dbId,
    apiClient.getCalibrationFramePreviewUrl(dbId, frame.frame_uuid, options),
    { kind: 'calibration_frame', frameUuid: frame.frame_uuid, ...options }
  );
  const grade = gradeLabel(frame);
  const state = frame.kept_out ? 'kept-out' : frame.grade ?? 'ungraded';
  return (
    <li>
      <button
        type="button"
        className={`calibration-thumb calibration-thumb-${state}`}
        aria-pressed={selected}
        aria-label={`Frame ${index + 1}: ${fileName(frame.source_path)}`}
        title={`${fileName(frame.source_path)}\n${formatTime(frame.captured_at)}${grade ? `\n${grade}` : ''}${frame.kept_out ? `\nKept out: ${frame.kept_out}` : ''}`}
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
              : frame.grade === 'suspect' ? <span className="calibration-badge calibration-badge-suspect">Check</span>
                : null}
        </span>
      </button>
    </li>
  );
}

interface CalibrationFrameStripProps {
  dbId: string;
  frames: BrowsedCalibrationFrame[];
  selected: string | null;
  stretch: CalibrationFrameStretch;
  onSelect: (uuid: string) => void;
}

/** The frames of one session in capture order, as small pictures. */
export default function CalibrationFrameStrip({ dbId, frames, selected, stretch, onSelect }: CalibrationFrameStripProps) {
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
      {frames.map((frame, index) => (
        <Thumb
          key={frame.frame_uuid}
          dbId={dbId}
          frame={frame}
          index={index}
          stretch={stretch}
          selected={frame.frame_uuid === selected}
          onSelect={onSelect}
        />
      ))}
    </ol>
  );
}
