import { useEffect, type ReactNode } from 'react';
import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../../api/client';
import type {
  BrowsedCalibrationFrame,
  CalibrationFrameStretch,
  CalibrationFrameView,
} from '../../api/types';
import CalibrationFramePanel from './CalibrationFramePanel';
import CalibrationFrameStrip, { type FrameRun } from './CalibrationFrameStrip';
import CalibrationFrameViewer from './CalibrationFrameViewer';
import { fileName } from './calibrationLabels';
import type { useCalibrationActions } from './useCalibrationActions';

interface CalibrationFrameBrowserProps {
  dbId: string;
  /** The frames to step through, in the order shown. */
  frames: BrowsedCalibrationFrame[];
  /** The frames are in, not the last list's while the next loads: a
   *  frame they drop gives way to the first. */
  ready: boolean;
  frameUuid: string | null;
  onSelectFrame: (uuid: string | null) => void;
  /** What the viewer keeps its zoom across: a session, or a flat group. */
  zoomKey: string;
  /** Divide the strip into runs, such as the nights of a flat group. */
  runOf?: (frame: BrowsedCalibrationFrame) => FrameRun;
  view: CalibrationFrameView;
  stretch: CalibrationFrameStretch;
  full: boolean;
  onView: (view: CalibrationFrameView) => void;
  onStretch: (stretch: CalibrationFrameStretch) => void;
  onFull: (full: boolean) => void;
  canManage: boolean;
  actions: ReturnType<typeof useCalibrationActions>;
  /** ← → and C answer: off while a master is open. */
  keysEnabled: boolean;
  /** What ↑ ↓ move through beside it, for the hint: sessions, groups. */
  upDown: string;
  /** Shown when the frames are in and there are none. */
  empty?: ReactNode;
}

/**
 * The Calibration page's image browser: a strip of the frames as small
 * pictures, the chosen one large with its stretch, comparison and zoom,
 * and its numbers beside it. A session's frames and a flat group's, night
 * by night, are browsed with the same one.
 */
export default function CalibrationFrameBrowser({
  dbId,
  frames,
  ready,
  frameUuid,
  onSelectFrame,
  zoomKey,
  runOf,
  view,
  stretch,
  full,
  onView,
  onStretch,
  onFull,
  canManage,
  actions,
  keysEnabled,
  upDown,
  empty,
}: CalibrationFrameBrowserProps) {
  // The last frame's detail stays, faded, while the next one loads, so the
  // panel keeps its height.
  const detail = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'frame', frameUuid],
    queryFn: () => apiClient.getCalibrationFrame(dbId, frameUuid!),
    enabled: !!frameUuid,
    placeholderData: keepPreviousData,
  });

  // The first frame is chosen until a person chooses; a frame the list
  // drops gives way to the first one left.
  useEffect(() => {
    if (!ready || frames.length === 0) return;
    if (!frameUuid || !frames.some((frame) => frame.frame_uuid === frameUuid)) {
      onSelectFrame(frames[0].frame_uuid);
    }
  }, [frameUuid, frames, onSelectFrame, ready]);

  const frameIndex = frames.findIndex((frame) => frame.frame_uuid === frameUuid);
  const current = detail.data && detail.data.frame_uuid === frameUuid ? detail.data : undefined;
  const shownDetail = frameUuid ? detail.data : undefined;
  // Whether the frame can be set against its session. While the next
  // frame's detail loads, the last one of the same session answers, so
  // stepping through a session in compare mode stays in it.
  const ownSession = frames[frameIndex]?.session;
  const compare = (current ?? (shownDetail?.session === ownSession ? shownDetail : undefined))?.compare ?? false;
  const spots = current?.measurement?.type === 'flat' && current.measurement.features
    ? { width: current.measurement.width, height: current.measurement.height, features: current.measurement.features }
    : undefined;

  const hotkeys = { enabled: keysEnabled, preventDefault: true };
  useHotkeys('left', () => {
    if (frameIndex > 0) onSelectFrame(frames[frameIndex - 1].frame_uuid);
  }, hotkeys, [frameIndex, frames, onSelectFrame]);
  useHotkeys('right', () => {
    if (frameIndex >= 0 && frameIndex < frames.length - 1) onSelectFrame(frames[frameIndex + 1].frame_uuid);
  }, hotkeys, [frameIndex, frames, onSelectFrame]);
  useHotkeys('c', () => {
    if (compare) onView(view === 'session' ? 'frame' : 'session');
  }, { enabled: keysEnabled }, [compare, onView, view]);

  const nextAfterForget = () => {
    const next = frames[frameIndex + 1] ?? frames[frameIndex - 1];
    onSelectFrame(next ? next.frame_uuid : null);
  };

  return (
    <>
      {ready && frames.length === 0 && empty}
      {frames.length > 0 && (
        <CalibrationFrameStrip dbId={dbId} frames={frames} selected={frameUuid}
          stretch={stretch} onSelect={onSelectFrame} runOf={runOf} />
      )}
      {frameUuid && (
        <div className="calibration-frame">
          <CalibrationFrameViewer
            dbId={dbId}
            frameUuid={frameUuid}
            sessionId={zoomKey}
            kind={current?.kind ?? frames[frameIndex]?.kind ?? 'flat'}
            label={current ? fileName(current.source_path) : 'Calibration frame'}
            view={view}
            stretch={stretch}
            full={full}
            canCompare={compare}
            spots={spots}
            onView={onView}
            onStretch={onStretch}
            onFull={onFull}
          />
          <CalibrationFramePanel
            detail={shownDetail}
            stale={!current}
            loading={detail.isLoading}
            error={detail.error ? detail.error.message : null}
            canManage={canManage}
            actions={actions}
            onForgotten={nextAfterForget}
          />
        </div>
      )}
      {frameIndex >= 0 && frames.length > 1 && (
        <p className="calibration-muted calibration-hint">
          Frame {frameIndex + 1} of {frames.length} · ← → frames · ↑ ↓ {upDown}{compare ? ' · C compare' : ''}
        </p>
      )}
    </>
  );
}
