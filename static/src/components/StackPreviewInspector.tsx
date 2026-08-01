import { useEffect, useMemo, useState } from 'react';
import type {
  MouseEvent as ReactMouseEvent,
  TouchEvent as ReactTouchEvent,
} from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import type {
  ArtifactSearchJob,
  ArtifactSearchResult,
  ReferenceRegion,
  ResidualFlatJob,
} from '../api/types';
import { useImageZoom } from '../hooks/useImageZoom';
import { canBuildResidualFlat, morphologyLabel } from './artifactMorphology';
import {
  artifactRegionFromPoints,
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
  eyebrow: string;
  title: string;
  label: string;
  summary: string[];
  imageUrl: string;
  fitsUrl: string;
  imageAlt: string;
  downloadLabel: string;
  artifactSource?: StackArtifactSource;
  artifactEnabled?: boolean;
  onOpenImage?: (imageId: number) => void;
  onClose: () => void;
}

const terminalJobStates = new Set(['completed', 'failed']);

function jobProgress(job: Pick<ArtifactSearchJob | ResidualFlatJob, 'state' | 'total_work_units' | 'completed_work_units'>): number {
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
  onOpenImage,
  onClose,
}: StackPreviewInspectorProps) {
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState(false);
  const [dimensions, setDimensions] = useState<{ width: number; height: number } | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [dragStart, setDragStart] = useState<ImagePoint | null>(null);
  const [dragEnd, setDragEnd] = useState<ImagePoint | null>(null);
  const [region, setRegion] = useState<ReferenceRegion | null>(null);
  const [regionError, setRegionError] = useState<string | null>(null);
  const [searchId, setSearchId] = useState<string | null>(null);
  const [correctionId, setCorrectionId] = useState<string | null>(null);
  const [showCorrected, setShowCorrected] = useState(false);
  const zoom = useImageZoom({ minScale: 0.05, maxScale: 10 });
  useHotkeys('escape', () => {
    if (selecting) {
      setSelecting(false);
      setDragStart(null);
      setDragEnd(null);
      return;
    }
    onClose();
  }, { enableOnFormTags: true }, [onClose, selecting]);
  useHotkeys('plus,equal', zoom.zoomIn, [zoom.zoomIn]);
  useHotkeys('minus', zoom.zoomOut, [zoom.zoomOut]);
  useHotkeys('0,f', zoom.zoomToFit, [zoom.zoomToFit]);
  useHotkeys('1', zoom.zoomTo100, [zoom.zoomTo100]);

  useEffect(() => {
    zoom.containerRef.current?.focus();
  }, [zoom.containerRef]);

  useEffect(() => {
    setSelecting(false);
    setDragStart(null);
    setDragEnd(null);
    setRegion(null);
    setRegionError(null);
    setSearchId(null);
    setCorrectionId(null);
    setShowCorrected(false);
  }, [imageUrl]);

  const startSearch = useMutation({
    mutationFn: async (selectedRegion: ReferenceRegion) => {
      if (!artifactSource) throw new Error('This preview has no source-frame provenance');
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
      return state && terminalJobStates.has(state) ? false : 500;
    },
  });
  const activeSearch = searchId ? (search.data ?? startSearch.data) : undefined;

  const startCorrection = useMutation({
    mutationFn: async (imageId: number) => {
      if (!artifactSource || artifactSource.kind !== 'mono' || !activeSearch) {
        throw new Error('Dust correction requires one mono source stack');
      }
      return apiClient.startResidualFlat(
        artifactSource.dbId,
        activeSearch.search_id,
        imageId
      );
    },
    onSuccess: (job) => {
      setCorrectionId(job.correction_id);
      setShowCorrected(false);
    },
  });

  const correction = useQuery({
    queryKey: ['db', artifactSource?.dbId, 'stack-residual-flat', correctionId],
    queryFn: () => apiClient.getResidualFlat(artifactSource!.dbId, correctionId!),
    enabled: !!artifactSource && correctionId !== null,
    initialData: correctionId && startCorrection.data?.correction_id === correctionId
      ? startCorrection.data
      : undefined,
    refetchInterval: (query) => {
      const state = query.state.data?.state;
      return state && terminalJobStates.has(state) ? false : 500;
    },
  });
  const activeCorrection = correctionId
    ? (correction.data ?? startCorrection.data)
    : undefined;
  const correctedReady = activeCorrection?.state === 'completed' && !!artifactSource;
  const activeImageUrl = correctedReady && showCorrected
    ? apiClient.getResidualFlatPreviewUrl(
      artifactSource.dbId,
      activeCorrection.correction_id,
      'original'
    )
    : imageUrl;
  const activeFitsUrl = correctedReady && showCorrected
    ? apiClient.getResidualFlatFitsUrl(artifactSource.dbId, activeCorrection.correction_id)
    : fitsUrl;

  useEffect(() => {
    setLoaded(false);
    setError(false);
    setDimensions(null);
    setSelecting(false);
    setDragStart(null);
    setDragEnd(null);
    setRegion(null);
    setRegionError(null);
  }, [activeImageUrl]);

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
      setRegionError(`Choose a region from ${MIN_ARTIFACT_REGION_EDGE} to ${MAX_ARTIFACT_REGION_EDGE} pixels on each side.`);
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
    event.preventDefault();
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
    event.preventDefault();
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

  return (
    <div className="stack-inspector-overlay" role="presentation" onClick={onClose}>
      <section
        className={`stack-inspector ${activeSearch ? 'with-artifact-results' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="stack-inspector-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="stack-inspector-header">
          <div>
            <div className="stack-preview-eyebrow">{eyebrow}</div>
            <h2 id="stack-inspector-title">
              {title} <span>{showCorrected ? `${label} · dust-corrected` : label}</span>
            </h2>
          </div>
          <div className="stack-inspector-summary">
            {summary.map((item) => <span key={item}>{item}</span>)}
            {dimensions && <span>{dimensions.width} × {dimensions.height}</span>}
          </div>
          <button className="close-button" type="button" onClick={onClose} aria-label="Close stack inspector">
            ×
          </button>
        </header>

        <div className="stack-inspector-body">
          <div
            className={`stack-inspector-canvas zoom-container ${zoom.hasOverflow ? 'has-overflow' : ''} ${selecting ? 'selecting-artifact-region' : ''}`}
            ref={zoom.containerRef}
            onWheel={zoom.handleWheel}
            onMouseDown={beginRegion}
            onMouseMove={moveRegion}
            onMouseUp={finishRegion}
            onMouseLeave={(event) => {
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
            {!loaded && !error && (
              <div className="stack-inspector-loading">
                <span className="stack-preview-spinner" aria-hidden="true" />
                Loading full-resolution stack…
              </div>
            )}
            {error ? (
              <div className="stack-inspector-loading error" role="alert">
                The full-resolution stack could not be loaded.
              </div>
            ) : (
              <img
                ref={zoom.imageRef}
                src={activeImageUrl}
                alt={imageAlt}
                data-testid="stack-inspector-image"
                draggable={false}
                onError={() => setError(true)}
                onLoad={(event) => {
                  const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
                  if (!width || !height) return;
                  setDimensions({ width, height });
                  zoom.setImageDimensions(width, height, true);
                  zoom.applyBitmapDimensions(width, height, 'fit');
                  setLoaded(true);
                }}
                style={{
                  visibility: loaded ? 'visible' : 'hidden',
                  transform: `translate(${zoom.zoomState.offsetX}px, ${zoom.zoomState.offsetY}px) scale(${zoom.zoomState.scale})`,
                  transformOrigin: '0 0',
                  cursor: selecting ? 'crosshair' : (zoom.hasOverflow ? 'grab' : 'default'),
                }}
              />
            )}
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
                    <span style={{ width: `${jobProgress(activeSearch)}%` }} />
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
                          artifactSource!.dbId,
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
                        {artifactSource
                          && canBuildResidualFlat(result, artifactSource.kind)
                          && (
                            <button
                              type="button"
                              disabled={startCorrection.isPending}
                              onClick={() => {
                                setCorrectionId(null);
                                setShowCorrected(false);
                                startCorrection.reset();
                                startCorrection.mutate(result.image_id);
                              }}
                            >
                              {startCorrection.isPending
                                && startCorrection.variables === result.image_id
                                ? 'Starting correction…'
                                : 'Build dust-corrected preview'}
                            </button>
                          )}
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
              {activeCorrection && (
                <section className="stack-residual-flat" aria-label="Dust-correction preview">
                  <header>
                    <div>
                      <div className="stack-preview-eyebrow">Experimental correction</div>
                      <h4>Detector-space dust residual</h4>
                    </div>
                    <span>{activeCorrection.filter_name}</span>
                  </header>
                  {activeCorrection.state !== 'completed' && activeCorrection.state !== 'failed' && (
                    <div className="stack-artifact-progress" role="status">
                      <div>
                        <span>{activeCorrection.phase}</span>
                        <strong>
                          {activeCorrection.completed_work_units} / {activeCorrection.total_work_units || '…'}
                        </strong>
                      </div>
                      <div className="stack-preview-progress-track">
                        <span style={{ width: `${jobProgress(activeCorrection)}%` }} />
                      </div>
                    </div>
                  )}
                  {activeCorrection.error && (
                    <div className="stack-preview-message error">{activeCorrection.error}</div>
                  )}
                  {activeCorrection.notes.map((note) => (
                    <div className="stack-artifact-note" key={note}>{note}</div>
                  ))}
                  {correctedReady && activeCorrection.diagnostics && artifactSource && (
                    <>
                      <img
                        className="stack-residual-flat-response"
                        src={apiClient.getResidualFlatResponseUrl(
                          artifactSource.dbId,
                          activeCorrection.correction_id
                        )}
                        alt="Estimated residual-flat response; dark areas receive correction"
                      />
                      <p>
                        {activeCorrection.sample_count} frames ·{' '}
                        {activeCorrection.dither_span_pixels?.toFixed(1)}px dither span ·{' '}
                        {activeCorrection.diagnostics.maximum_applied_gain.toFixed(3)}× peak gain
                      </p>
                      <p>
                        {((activeCorrection.diagnostics.corrected_samples
                          / activeCorrection.diagnostics.total_samples) * 100).toFixed(2)}% of the patch corrected ·{' '}
                        {activeCorrection.diagnostics.largest_connected_pixels} connected pixels ·{' '}
                        {activeCorrection.accepted_frames} frames stacked
                      </p>
                      <div className="stack-residual-flat-actions">
                        <button type="button" onClick={() => setShowCorrected((current) => !current)}>
                          {showCorrected ? 'Show original stack' : 'Show corrected stack'}
                        </button>
                        <a
                          href={apiClient.getResidualFlatFitsUrl(
                            artifactSource.dbId,
                            activeCorrection.correction_id
                          )}
                          download
                        >
                          Download corrected FITS
                        </a>
                      </div>
                    </>
                  )}
                </section>
              )}
              {startCorrection.error && !activeCorrection && (
                <div className="stack-preview-message error">
                  {startCorrection.error instanceof Error
                    ? startCorrection.error.message
                    : 'Dust correction could not start'}
                </div>
              )}
            </aside>
          )}
        </div>

        <footer className="stack-inspector-toolbar">
          <div className="stack-inspector-hint">
            {selecting
              ? `Drag a ${MIN_ARTIFACT_REGION_EDGE}–${MAX_ARTIFACT_REGION_EDGE}px box over the artifact`
              : 'Wheel to zoom · drag to pan · F fit · 1 actual size'}
          </div>
          {regionError && <span className="stack-artifact-region-error" role="alert">{regionError}</span>}
          {artifactSource && (
            <>
              <button
                className={`stack-artifact-select ${selecting ? 'active' : ''}`}
                type="button"
                disabled={!loaded || !artifactEnabled || showCorrected || startSearch.isPending}
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
                  disabled={!artifactEnabled || showCorrected || startSearch.isPending}
                  onClick={() => {
                    setSearchId(null);
                    startSearch.reset();
                    startSearch.mutate(region);
                  }}
                >
                  {startSearch.isPending ? 'Starting…' : 'Search this region'}
                </button>
              )}
            </>
          )}
          {startSearch.error && (
            <span className="stack-artifact-region-error" role="alert">
              {startSearch.error instanceof Error ? startSearch.error.message : 'Search failed'}
            </span>
          )}
          <a className="stack-preview-download" href={activeFitsUrl} download>
            {showCorrected ? 'Download corrected FITS' : downloadLabel}
          </a>
          <div className="zoom-info-compact">
            <span className="zoom-percentage-compact">{zoom.getZoomPercentage()}%</span>
          </div>
          <div className="zoom-buttons-compact stack-inspector-zoom-buttons">
            <button className="zoom-btn-compact" type="button" onClick={zoom.zoomOut} title="Zoom Out (-)">−</button>
            <button className="zoom-btn-compact" type="button" onClick={zoom.zoomToFit} title="Fit to Screen (F)">Fit</button>
            <button className="zoom-btn-compact" type="button" onClick={zoom.zoomTo100} title="100% (1)">100%</button>
            <button className="zoom-btn-compact" type="button" onClick={zoom.zoomIn} title="Zoom In (+)">+</button>
          </div>
        </footer>
      </section>
    </div>
  );
}
