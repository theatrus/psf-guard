import { useEffect, useRef, useState, type ReactNode } from 'react';
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
import { useRigClocks } from './useRigClocks';

interface CalibrationFrameBrowserProps {
  dbId: string;
  /** The frames to step through, in the order shown. */
  frames: BrowsedCalibrationFrame[];
  /** The frames are in, not the last list's while the next loads: a
   *  frame they drop gives way to the first. */
  ready: boolean;
  frameUuid: string | null;
  onSelectFrame: (uuid: string | null) => void;
  /** Choose `fallback` unless the frame chosen now, which may not have
   *  rendered yet, is one of `listed`. */
  onKeepListedFrame: (listed: readonly string[], fallback: string | null) => void;
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
  /** Spots drawn over the frame, and nested under it, large dust. */
  showSpots: boolean;
  showDust: boolean;
  onShowSpots: (show: boolean) => void;
  onShowDust: (show: boolean) => void;
  canManage: boolean;
  actions: ReturnType<typeof useCalibrationActions>;
  /** ← → and C answer: off while a master is open. */
  keysEnabled: boolean;
  /** What ↑ ↓ move through beside it, for the hint: sessions, groups. */
  upDown: string;
  /** Shown when the frames are in and there are none. */
  empty?: ReactNode;
  /** Said above the strip, such as that it holds only the first frames. */
  note?: ReactNode;
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
  onKeepListedFrame,
  zoomKey,
  runOf,
  view,
  stretch,
  full,
  onView,
  onStretch,
  onFull,
  showSpots,
  showDust,
  onShowSpots,
  onShowDust,
  canManage,
  actions,
  keysEnabled,
  upDown,
  empty,
  note,
}: CalibrationFrameBrowserProps) {
  // The last frame's detail stays, faded, while the next one loads, so the
  // panel keeps its height.
  const clockOf = useRigClocks(dbId);
  const detail = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'frame', frameUuid],
    queryFn: () => apiClient.getCalibrationFrame(dbId, frameUuid!),
    enabled: !!frameUuid,
    placeholderData: keepPreviousData,
  });

  // Forget a frame, and show its neighbour after it: the next in the
  // strip, else the one before. The neighbour is chosen when the request
  // answers and again when the list drops the frame, whichever comes
  // first, so both choose the same frame. The list without the forgotten
  // frame can render before the address shows the first choice; the
  // effect below then still sees the forgotten frame, and without this
  // would choose the first frame left over the neighbour.
  const forgotten = useRef<{ uuid: string; next: string | null } | null>(null);
  const forget = (uuid: string) => {
    const index = frames.findIndex((frame) => frame.frame_uuid === uuid);
    const next = index < 0 ? undefined : frames[index + 1] ?? frames[index - 1];
    const after = { uuid, next: next?.frame_uuid ?? null };
    forgotten.current = after;
    actions.forget.mutate([uuid], {
      onSuccess: () => {
        // Unless another frame was chosen meanwhile.
        if (forgotten.current === after) onSelectFrame(after.next);
      },
    });
  };

  // The first frame is chosen until a person chooses; a frame the list
  // drops gives way to the first one left, or after Forget to its
  // neighbour, and an empty list chooses none, so Keep out and Forget
  // never reach a frame it does not show. The choice is made against the
  // frame the address holds now, which may be one chosen a moment ago
  // that this render does not show yet: that one stays if listed.
  const listed = frames.some((frame) => frame.frame_uuid === frameUuid);
  useEffect(() => {
    if (!ready) return;
    if (frameUuid && listed) return;
    if (frames.length === 0 && !frameUuid) return;
    const uuids = frames.map((frame) => frame.frame_uuid);
    const after = forgotten.current?.uuid === frameUuid ? forgotten.current : null;
    const neighbour = after?.next && uuids.includes(after.next) ? after.next : null;
    onKeepListedFrame(uuids, neighbour ?? uuids[0] ?? null);
  }, [frameUuid, frames, listed, onKeepListedFrame, ready]);
  // Another frame chosen ends what Forget asked for.
  useEffect(() => {
    if (forgotten.current && frameUuid !== forgotten.current.uuid && frameUuid !== forgotten.current.next) {
      forgotten.current = null;
    }
  }, [frameUuid]);
  // A frame the list does not hold is not drawn, once the list is in.
  const shown = frameUuid && (listed || !ready) ? frameUuid : null;

  const frameIndex = frames.findIndex((frame) => frame.frame_uuid === frameUuid);
  const current = detail.data && detail.data.frame_uuid === frameUuid ? detail.data : undefined;
  const shownDetail = frameUuid ? detail.data : undefined;
  // Whether the frame can be set against its session. While the next
  // frame's detail loads, the last one of the same session answers, so
  // stepping through a session in compare mode stays in it.
  // The list says whether a frame compares with its session, so stepping
  // into another night in comparison stays in it while its detail loads.
  // Without that word, the last one known holds until the detail is in.
  const ownSession = frames[frameIndex]?.session;
  const known = frames[frameIndex]?.compare
    ?? (current ?? (shownDetail?.session === ownSession ? shownDetail : undefined))?.compare;
  const [held, setHeld] = useState(false);
  if (known !== undefined && known !== held) setHeld(known);
  const compare = known ?? held;
  const spots = current?.measurement?.type === 'flat' && current.measurement.features
    ? {
        width: current.measurement.width,
        height: current.measurement.height,
        features: current.measurement.features,
        vignetting: current.measurement.vignetting,
      }
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

  return (
    <>
      {ready && frames.length === 0 && empty}
      {frames.length > 0 && note}
      {frames.length > 0 && (
        <CalibrationFrameStrip dbId={dbId} frames={frames} selected={frameUuid}
          stretch={stretch} onSelect={onSelectFrame} runOf={runOf} />
      )}
      {shown && (
        <div className="calibration-frame">
          <CalibrationFrameViewer
            dbId={dbId}
            frameUuid={shown}
            sessionId={zoomKey}
            kind={current?.kind ?? frames[frameIndex]?.kind ?? 'flat'}
            label={current ? fileName(current.source_path) : 'Calibration frame'}
            view={view}
            stretch={stretch}
            full={full}
            canCompare={compare}
            spots={spots}
            sessionDust={current?.session_dust}
            showSpots={showSpots}
            showDust={showDust}
            onShowSpots={onShowSpots}
            onShowDust={onShowDust}
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
            onForget={forget}
            clock={clockOf(shownDetail?.rig_uuid)}
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
