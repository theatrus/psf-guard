import { useEffect, useMemo, useRef, useState } from 'react';
import type {
  CSSProperties,
  MouseEvent as ReactMouseEvent,
  TouchEvent as ReactTouchEvent,
  ReactNode,
} from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import type { OverlaySolution } from '@seiza/astro-overlay';
import { apiClient } from '../api/client';
import type { ArtifactSearchJob, ArtifactSearchResult, PreviewDescriptor, ReferenceRegion } from '../api/types';
import { useImageZoom } from '../hooks/useImageZoom';
import { useAsyncImage } from '../hooks/useAsyncImage';
import DetailSection from './DetailSection';
import ImageViewerFrame from './viewer/ImageViewerFrame';
import PointerReadout from './viewer/PointerReadout';
import { ViewerShortcuts, ViewerZoomSection } from './viewer/ViewerControls';
import { useViewerReadout } from './viewer/useViewerReadout';
import { useViewerZoomHotkeys } from './viewer/useViewerZoomHotkeys';
import { morphologyLabel } from './artifactMorphology';
import {
  artifactRegionFromPoints,
  isArtifactRegionCapped,
  MAX_ARTIFACT_REGION_EDGE,
  MIN_ARTIFACT_REGION_EDGE,
} from './stackArtifactRegion';
import type { ImagePoint } from './stackArtifactRegion';

export type StackArtifactSource =
  | {
      kind: 'mono';
      dbId: string;
      jobId: string;
      groupIndex: number;
      artifactRevision: string;
    }
  | {
      kind: 'color';
      dbId: string;
      jobId: string;
      artifactRevision: string;
    };

interface StackPreviewInspectorProps {
  className?: string;
  eyebrow: string;
  title: string;
  label: string;
  summary: string[];
  imageUrl: string | null;
  fitsUrl: string | null;
  imageAlt: string;
  downloadLabel: string;
  artifactSource?: StackArtifactSource;
  artifactEnabled?: boolean;
  controls?: ReactNode;
  asyncPreview?: { dbId: string; descriptor: PreviewDescriptor };
  imageIdentity?: string;
  emptyMessage?: string;
  loadingMessage?: string;
  errorMessage?: string;
  closeLabel?: string;
  /**
   * The plate solution of the stack's grid, for RA/Dec under the pointer:
   * null while none is known, absent for an image that has no sky position,
   * such as a calibration master.
   */
  solution?: OverlaySolution | null;
  /** Why RA/Dec cannot be shown, when `solution` is null. */
  solutionUnavailable?: string;
  onOpenImage?: (imageId: number) => void;
  onClose: () => void;
}

const terminalSearchStates = new Set(['completed', 'failed']);

function searchProgress(job: ArtifactSearchJob): number {
  if (job.state === 'completed') return 100;
  if (!job.total_work_units) return 0;
  return Math.min(100, Math.round((job.completed_work_units / job.total_work_units) * 100));
}

function formatCaptureTime(timestamp: number | null): string {
  if (timestamp == null) return 'Capture time unknown';
  return new Date(timestamp * 1000).toLocaleString();
}

function gradeLabel(status: number): string {
  if (status === 1) return 'Accepted';
  if (status === 2) return 'Rejected';
  return 'Pending';
}

function ArtifactMorphologyBadge({ result }: { result: ArtifactSearchResult }) {
  const label = morphologyLabel(result);
  return label ? <p className="stack-artifact-morphology">{label}</p> : null;
}

export default function StackPreviewInspector({
  className = '',
  eyebrow,
  title,
  label,
  summary,
  imageUrl,
  fitsUrl,
  imageAlt,
  downloadLabel,
  artifactSource,
  artifactEnabled = false,
  controls,
  asyncPreview,
  imageIdentity,
  emptyMessage,
  loadingMessage = 'Loading full-resolution stack…',
  errorMessage = 'The full-resolution stack could not be loaded.',
  closeLabel = 'Close stack inspector',
  solution,
  solutionUnavailable = 'No integrated frame of this stack is plate-solved yet',
  onOpenImage,
  onClose,
}: StackPreviewInspectorProps) {
  const image = useAsyncImage(asyncPreview?.dbId, imageUrl ?? '', asyncPreview?.descriptor);
  const loaded = image.state === 'ready';
  const error = image.state === 'error';
  const loadedIdentity = useRef<string | null>(null);
  const [dimensions, setDimensions] = useState<{ width: number; height: number } | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [dragStart, setDragStart] = useState<ImagePoint | null>(null);
  const [dragEnd, setDragEnd] = useState<ImagePoint | null>(null);
  const [region, setRegion] = useState<ReferenceRegion | null>(null);
  const [regionError, setRegionError] = useState<string | null>(null);
  const [searchId, setSearchId] = useState<string | null>(null);
  const zoom = useImageZoom({ minScale: 0.05, maxScale: 10 });
  const readout = useViewerReadout({
    container: zoom.containerRef,
    view: zoom.zoomState,
    bitmap: dimensions,
    windowPath: fitsUrl ? apiClient.fitsWindowPath(fitsUrl) : null,
    solution,
  });
  const skyUnavailable = solution === undefined
    ? 'This image has no sky position'
    : solution?.wcs ? undefined : solutionUnavailable;
  useHotkeys('escape', () => {
    if (selecting) {
      setSelecting(false);
      setDragStart(null);
      setDragEnd(null);
      return;
    }
    onClose();
  }, { enableOnFormTags: true }, [onClose, selecting]);
  useViewerZoomHotkeys(zoom);

  useEffect(() => {
    zoom.containerRef.current?.focus();
  }, [zoom.containerRef]);

  useEffect(() => {
    setDimensions(null);
    setSelecting(false);
    setDragStart(null);
    setDragEnd(null);
    setRegion(null);
    setRegionError(null);
    setSearchId(null);
  }, [imageUrl]);

  const startSearch = useMutation({
    mutationFn: async (selectedRegion: ReferenceRegion) => {
      if (!artifactSource) throw new Error('This preview has no record of its input frames');
      if (artifactSource.kind === 'mono') {
        return apiClient.startMonoArtifactSearch(
          artifactSource.dbId,
          artifactSource.jobId,
          artifactSource.groupIndex,
          artifactSource.artifactRevision,
          selectedRegion
        );
      }
      return apiClient.startColorArtifactSearch(
        artifactSource.dbId,
        artifactSource.jobId,
        artifactSource.artifactRevision,
        selectedRegion
      );
    },
    onSuccess: (job) => setSearchId(job.search_id),
  });

  const search = useQuery({
    queryKey: ['db', artifactSource?.dbId, 'stack-artifact-search', searchId],
    queryFn: () => apiClient.getArtifactSearch(artifactSource!.dbId, searchId!),
    enabled: !!artifactSource && searchId !== null,
    initialData: searchId && startSearch.data?.search_id === searchId ? startSearch.data : undefined,
    refetchInterval: (query) => {
      const state = query.state.data?.state;
      return state && terminalSearchStates.has(state) ? false : 500;
    },
  });
  const activeSearch = searchId ? (search.data ?? startSearch.data) : undefined;

  const resultsByFilter = useMemo(() => {
    const groups = new Map<string, ArtifactSearchJob['results']>();
    for (const result of activeSearch?.results ?? []) {
      const current = groups.get(result.filter_name) ?? [];
      current.push(result);
      groups.set(result.filter_name, current);
    }
    return [...groups.entries()];
  }, [activeSearch?.results]);
  const hasSuspect = activeSearch?.results.some((result) => result.evidence !== 'low') ?? false;

  const imagePointFromClient = (clientX: number, clientY: number): ImagePoint | null => {
    if (!dimensions || !zoom.containerRef.current) return null;
    const bounds = zoom.containerRef.current.getBoundingClientRect();
    return {
      x: Math.max(0, Math.min(
        dimensions.width,
        (clientX - bounds.left - zoom.zoomState.offsetX) / zoom.zoomState.scale
      )),
      y: Math.max(0, Math.min(
        dimensions.height,
        (clientY - bounds.top - zoom.zoomState.offsetY) / zoom.zoomState.scale
      )),
    };
  };

  const imagePoint = (event: ReactMouseEvent<HTMLDivElement>): ImagePoint | null =>
    imagePointFromClient(event.clientX, event.clientY);

  const finishRegionAt = (point: ImagePoint) => {
    if (!dragStart || !dimensions) return;
    const selected = artifactRegionFromPoints(
      dragStart,
      point,
      dimensions.width,
      dimensions.height
    );
    setDragEnd(point);
    setDragStart(null);
    if (!selected) {
      setRegion(null);
      setRegionError(`Drag a box at least ${MIN_ARTIFACT_REGION_EDGE} pixels on each side.`);
      return;
    }
    setRegion(selected);
    setRegionError(null);
    setSelecting(false);
  };

  const beginRegion = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (!selecting) {
      zoom.handleMouseDown(event);
      return;
    }
    const point = imagePoint(event);
    if (!point) return;
    event.preventDefault();
    setDragStart(point);
    setDragEnd(point);
    setRegion(null);
    setRegionError(null);
  };

  const moveRegion = (event: ReactMouseEvent<HTMLDivElement>) => {
    readout.track(event);
    if (!selecting || !dragStart) {
      zoom.handleMouseMove(event);
      return;
    }
    const point = imagePoint(event);
    if (point) setDragEnd(point);
  };

  const finishRegion = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (!selecting || !dragStart || !dimensions) {
      zoom.handleMouseUp(event);
      return;
    }
    const point = imagePoint(event) ?? dragEnd ?? dragStart;
    finishRegionAt(point);
  };

  const beginTouchRegion = (event: ReactTouchEvent<HTMLDivElement>) => {
    if (!selecting || event.touches.length !== 1) {
      if (event.touches.length >= 2) {
        setDragStart(null);
        setDragEnd(null);
      }
      zoom.handleTouchStart(event);
      return;
    }
    const touch = event.touches[0];
    const point = imagePointFromClient(touch.clientX, touch.clientY);
    if (!point) return;
    setDragStart(point);
    setDragEnd(point);
    setRegion(null);
    setRegionError(null);
  };

  const moveTouchRegion = (event: ReactTouchEvent<HTMLDivElement>) => {
    if (!selecting || !dragStart || event.touches.length !== 1) {
      zoom.handleTouchMove(event);
      return;
    }
    const touch = event.touches[0];
    const point = imagePointFromClient(touch.clientX, touch.clientY);
    if (!point) return;
    setDragEnd(point);
  };

  const finishTouchRegion = (event: ReactTouchEvent<HTMLDivElement>) => {
    if (!selecting || !dragStart || event.touches.length !== 0) {
      zoom.handleTouchEnd(event);
      return;
    }
    event.preventDefault();
    finishRegionAt(dragEnd ?? dragStart);
  };

  const displayedRegion = region ?? (
    dragStart && dragEnd && dimensions
      ? artifactRegionFromPoints(dragStart, dragEnd, dimensions.width, dimensions.height)
      : null
  );

  const canvas = (
    <div
      className={`image-container zoom-container stack-inspector-canvas ${zoom.hasOverflow ? 'has-overflow' : ''} ${selecting ? 'selecting-artifact-region' : ''}`}
      ref={zoom.containerRef}
      onWheel={(event) => {
        zoom.handleWheel(event);
        readout.track(event);
      }}
      onMouseDown={beginRegion}
      onMouseMove={moveRegion}
      onMouseUp={finishRegion}
      onMouseLeave={(event) => {
        readout.leave();
        if (dragStart) finishRegion(event);
        else zoom.handleMouseUp(event);
      }}
      onTouchStart={beginTouchRegion}
      onTouchMove={moveTouchRegion}
      onTouchEnd={finishTouchRegion}
      onTouchCancel={finishTouchRegion}
      onKeyDown={zoom.handleKeyDown}
      tabIndex={0}
    >
      {!imageUrl ? (
        <div className="stack-inspector-loading" role="status">{emptyMessage}</div>
      ) : !loaded && !error && (
        <div className="stack-inspector-loading">
          <span className="stack-preview-spinner" aria-hidden="true" />
          {image.state === 'generating' ? 'Generating preview…' : loadingMessage}
        </div>
      )}
      {error ? (
        <div className="stack-inspector-loading error" role="alert">
          {image.error || errorMessage}
        </div>
      ) : imageUrl && (
        <img
          ref={zoom.imageRef}
          src={image.src}
          alt={imageAlt}
          data-testid="stack-inspector-image"
          draggable={false}
          onError={image.onError}
          onLoad={(event) => {
            const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
            if (!width || !height) return;
            setDimensions({ width, height });
            zoom.setImageDimensions(width, height, true);
            const identity = imageIdentity ?? imageUrl;
            zoom.applyBitmapDimensions(width, height, loadedIdentity.current === identity ? 'preserve' : 'fit');
            loadedIdentity.current = identity;
            image.onLoad();
          }}
          style={{
            visibility: loaded ? 'visible' : 'hidden',
            transform: `translate(${zoom.zoomState.offsetX}px, ${zoom.zoomState.offsetY}px) scale(${zoom.zoomState.scale})`,
            transformOrigin: '0 0',
            cursor: selecting || readout.active ? 'crosshair' : (zoom.hasOverflow ? 'grab' : 'default'),
          }}
        />
      )}
      <PointerReadout {...readout.readout} skyUnavailable={skyUnavailable} />
      {displayedRegion && (
        <div
          className="stack-artifact-region"
          data-testid="stack-artifact-region"
          style={{
            left: zoom.zoomState.offsetX + displayedRegion.x * zoom.zoomState.scale,
            top: zoom.zoomState.offsetY + displayedRegion.y * zoom.zoomState.scale,
            width: displayedRegion.width * zoom.zoomState.scale,
            height: displayedRegion.height * zoom.zoomState.scale,
          }}
        />
      )}
    </div>
  );

  return (
    <ImageViewerFrame
      dialog
      overlayClassName="stack-inspector-overlay"
      className={`stack-inspector ${className}`.trim()}
      title={<>{title} <span>{label}</span></>}
      closeLabel={closeLabel}
      onClose={onClose}
      image={canvas}
      imageStyle={dimensions
        ? { '--detail-frame-aspect': String(dimensions.width / dimensions.height) } as CSSProperties
        : undefined}
    >
      <DetailSection title={eyebrow} open className="stack-inspector-details">
        <div className="stack-inspector-summary">
          {summary.map((item) => <span key={item}>{item}</span>)}
          {dimensions && <span>{dimensions.width} × {dimensions.height}</span>}
        </div>
        {fitsUrl && <a className="stack-preview-download" href={fitsUrl} download>{downloadLabel}</a>}
      </DetailSection>

      {controls}

      {artifactSource && (
        <DetailSection title="Source artifacts" open className="stack-artifact-section">
          <div className="stack-artifact-actions">
            <button
              className="stack-artifact-select"
              type="button"
              disabled={!loaded || !artifactEnabled || startSearch.isPending}
              aria-pressed={selecting}
              onClick={() => {
                setSelecting((current) => !current);
                setDragStart(null);
                setDragEnd(null);
                setRegionError(null);
              }}
            >
              {selecting ? 'Cancel selection' : 'Find source artifact'}
            </button>
            {region && (
              <button
                className="stack-artifact-search"
                type="button"
                disabled={!artifactEnabled || startSearch.isPending}
                onClick={() => {
                  setSearchId(null);
                  startSearch.reset();
                  startSearch.mutate(region);
                }}
              >
                {startSearch.isPending ? 'Starting…' : 'Search this region'}
              </button>
            )}
          </div>
          <div className="stack-inspector-hint">
            {!selecting
              ? 'Choose Find source artifact, then drag a box over the artifact to rank the frames that put it there.'
              : displayedRegion && isArtifactRegionCapped(displayedRegion)
                ? `${MAX_ARTIFACT_REGION_EDGE}px is as wide as the search looks — drag from another corner to move the box`
                : `Drag a box over the artifact, up to ${MAX_ARTIFACT_REGION_EDGE}px a side`}
          </div>
          {regionError && <span className="stack-artifact-region-error" role="alert">{regionError}</span>}
          {startSearch.error && (
            <span className="stack-artifact-region-error" role="alert">
              {startSearch.error instanceof Error ? startSearch.error.message : 'Search failed'}
            </span>
          )}
          {activeSearch && (
            <aside className="stack-artifact-results" aria-label="Source-frame search results">
              <header>
                <div>
                  <div className="stack-preview-eyebrow">Selected region</div>
                  <h3>Source-frame ranking</h3>
                </div>
                <span>{activeSearch.region.width} × {activeSearch.region.height}px</span>
              </header>
              {activeSearch.state !== 'completed' && activeSearch.state !== 'failed' && (
                <div className="stack-artifact-progress" role="status">
                  <div>
                    <span>{activeSearch.phase}</span>
                    <strong>{activeSearch.completed_work_units} / {activeSearch.total_work_units}</strong>
                  </div>
                  <div className="stack-preview-progress-track">
                    <span style={{ width: `${searchProgress(activeSearch)}%` }} />
                  </div>
                </div>
              )}
              {activeSearch.error && <div className="stack-preview-message error">{activeSearch.error}</div>}
              {activeSearch.notes.map((note) => (
                <div className="stack-artifact-note" key={note}>{note}</div>
              ))}
              {activeSearch.state === 'completed'
                && activeSearch.results.length > 0
                && !hasSuspect && (
                <div className="stack-artifact-note">
                  No source frame clearly separates from its peers in this region.
                </div>
              )}
              {hasSuspect && (
                <div className="stack-artifact-note">
                  Shape labels describe the changed pixels. They suggest a cause but do not prove one.
                </div>
              )}
              {resultsByFilter.map(([filterName, results]) => (
                <section className="stack-artifact-result-group" key={filterName}>
                  <h4>{filterName}</h4>
                  {results.map((result, index) => (
                    <article className={`stack-artifact-result ${result.evidence}`} key={result.image_id}>
                      <img
                        src={apiClient.getArtifactCropUrl(
                          artifactSource.dbId,
                          activeSearch.search_id,
                          result.image_id
                        )}
                        alt={`Selected source crop from image ${result.image_id}`}
                      />
                      <div>
                        <header>
                          <strong>#{index + 1} · Image {result.image_id}</strong>
                          <span>{result.evidence}</span>
                        </header>
                        <small>
                          {formatCaptureTime(result.acquired_unix_seconds)} · {gradeLabel(result.grading_status)}
                        </small>
                        <p>
                          {result.peak_sigma.toFixed(1)}σ peak ·{' '}
                          {((result.bright_fraction + result.dark_fraction) * 100).toFixed(2)}% changed ·{' '}
                          {result.direction}
                        </p>
                        <ArtifactMorphologyBadge result={result} />
                        {onOpenImage && (
                          <button type="button" onClick={() => {
                            onOpenImage(result.image_id);
                            onClose();
                          }}>
                            Inspect source image
                          </button>
                        )}
                      </div>
                    </article>
                  ))}
                </section>
              ))}
              {activeSearch.state === 'completed' && activeSearch.results.length === 0 && (
                <div className="stack-artifact-note">No source crops had enough common coverage.</div>
              )}
            </aside>
          )}
        </DetailSection>
      )}

      <ViewerShortcuts measure={readout.options}>
        <span>Wheel Zoom</span>
        <span>Drag Pan</span>
        <span>F Fit</span>
        <span>1 Actual size</span>
        <span>Esc Close</span>
      </ViewerShortcuts>

      <ViewerZoomSection
        percentage={zoom.getZoomPercentage()}
        onZoomOut={zoom.zoomOut}
        onFit={zoom.zoomToFit}
        onActualSize={zoom.zoomTo100}
        onZoomIn={zoom.zoomIn}
        options={readout.options}
        valueDisabled={fitsUrl ? undefined : 'This image has no linear file to read values from'}
        skyDisabled={skyUnavailable}
      />
    </ImageViewerFrame>
  );
}
