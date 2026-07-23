import {
  memo,
  useState,
  useMemo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
} from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import { useSequenceAnalysis } from '../hooks/useSequenceAnalysis';
import { useSpatialScan } from '../hooks/useSpatialScan';
import { useGrading } from '../hooks/useGrading';
import { useDbProjectTarget, useGridState } from '../hooks/useUrlState';
import UndoRedoToolbar from './UndoRedoToolbar';
import ImageCard from './ImageCard';
import Dialog from './Dialog';
import type { Image, ScoredSequence, ImageQualityResult } from '../api/types';

function formatCategory(category: string): string {
  return category
    .split('_')
    .map(w => w.charAt(0).toUpperCase() + w.slice(1))
    .join(' ');
}

function formatSequenceLabel(sequence: ScoredSequence): string {
  if (!sequence.session_start) return `${sequence.filter_name} (${sequence.image_count})`;
  const start = new Date(sequence.session_start * 1000);
  const when = start.toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
  return `${sequence.filter_name} · ${when} (${sequence.image_count})`;
}

function qualityColor(score: number): string {
  if (score >= 0.7) return 'var(--color-success)';
  if (score >= 0.5) return 'var(--color-warning)';
  return 'var(--color-error)';
}

export default function SequenceView() {
  const { dbId, projectId, targetId } = useDbProjectTarget();
  const { currentImageId: urlCurrentImageId, setCurrentImageId } = useGridState();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const grading = useGrading(dbId!);
  const { analyze, data: analysisData, isLoading: isAnalyzing, error: analysisError } = useSequenceAnalysis(dbId);

  const filterName = searchParams.get('filterName') || undefined;
  const [threshold, setThreshold] = useState(0.5);
  const [selectedImages, setSelectedImages] = useState<Set<number>>(new Set());
  const [showRejectReview, setShowRejectReview] = useState(false);
  const [activeSequenceIndex, setActiveSequenceIndex] = useState(0);
  const spatialScan = useSpatialScan(dbId, targetId ?? undefined, filterName);

  // Fetch targets for the project to allow selection
  const { data: targets = [] } = useQuery({
    queryKey: ['db', dbId, 'targets', projectId],
    queryFn: () => apiClient.getTargets(dbId!, projectId!),
    enabled: !!dbId && !!projectId,
  });

  // Fetch images for the target (for preview URLs)
  const { data: images = [] } = useQuery({
    queryKey: ['db', dbId, 'all-images', projectId, targetId],
    queryFn: () =>
      apiClient.getImages(dbId!, {
        project_id: projectId || undefined,
        target_id: targetId || undefined,
        limit: 10000,
      }),
    enabled: !!dbId && projectId !== undefined,
  });

  // Build image lookup map
  const imageMap = useMemo(() => {
    const map = new Map<number, typeof images[0]>();
    images.forEach(img => map.set(img.id, img));
    return map;
  }, [images]);

  // Auto-analyze when target is selected
  useEffect(() => {
    if (targetId) {
      analyze({ target_id: targetId, filter_name: filterName });
    }
  }, [targetId, filterName, analyze]);

  const sequences = analysisData?.sequences || [];
  const activeSequence: ScoredSequence | undefined = sequences[activeSequenceIndex];
  const activeImageId = useMemo(() => {
    if (!activeSequence || activeSequence.images.length === 0) return null;
    if (urlCurrentImageId
      && activeSequence.images.some(image => image.image_id === urlCurrentImageId)) {
      return urlCurrentImageId;
    }
    return activeSequence.images[0].image_id;
  }, [activeSequence, urlCurrentImageId]);
  const activeImageIdRef = useRef(activeImageId);
  activeImageIdRef.current = activeImageId;

  useEffect(() => {
    if (activeImageId !== null && activeImageId !== urlCurrentImageId) {
      setCurrentImageId(activeImageId);
    }
  }, [activeImageId, setCurrentImageId, urlCurrentImageId]);

  // Get unique filter names from available targets
  const availableFilters = useMemo(() => {
    const filters = new Set<string>();
    images.forEach(img => {
      if (img.filter_name) filters.add(img.filter_name);
    });
    return Array.from(filters).sort();
  }, [images]);

  // Select all images below threshold
  const selectBelowThreshold = useCallback(() => {
    if (!activeSequence) return;
    const ids = new Set<number>();
    activeSequence.images.forEach(img => {
      if (img.quality_score < threshold) {
        ids.add(img.image_id);
      }
    });
    setSelectedImages(ids);
  }, [activeSequence, threshold]);

  // Select contiguous runs of clouded/occluded/bad images
  const selectCloudedSequence = useCallback(() => {
    if (!activeSequence) return;
    const ids = new Set<number>();
    let inRun = false;
    const runBuffer: number[] = [];

    for (const img of activeSequence.images) {
      const isBad =
        img.category === 'likely_clouds' ||
        img.category === 'possible_obstruction' ||
        img.quality_score < 0.3;
      if (isBad) {
        inRun = true;
        runBuffer.push(img.image_id);
      } else {
        if (inRun && runBuffer.length >= 2) {
          runBuffer.forEach(id => ids.add(id));
        }
        inRun = false;
        runBuffer.length = 0;
      }
    }
    // Flush trailing run
    if (inRun && runBuffer.length >= 2) {
      runBuffer.forEach(id => ids.add(id));
    }
    setSelectedImages(ids);
  }, [activeSequence]);

  const selectAstrometryIssues = useCallback(() => {
    if (!activeSequence) return;
    setSelectedImages(new Set(
      activeSequence.images
        .filter(img => (img.flags ?? []).some(flag =>
          flag === 'off_target' || flag === 'pointing_jump' || flag === 'pointing_drift'))
        .map(img => img.image_id)
    ));
  }, [activeSequence]);

  const selectUnsolved = useCallback(() => {
    if (!activeSequence) return;
    setSelectedImages(new Set(
      activeSequence.images
        .filter(img => img.pointing?.solve_failed && img.pointing.image_quality_evidence)
        .map(img => img.image_id)
    ));
  }, [activeSequence]);

  const selectRecommended = useCallback(() => {
    if (!activeSequence) return;
    setSelectedImages(new Set(
      activeSequence.images
        .filter(img => !!img.regrade_reason)
        .map(img => img.image_id)
    ));
  }, [activeSequence]);

  const applySelectionPreset = useCallback((preset: string) => {
    switch (preset) {
      case 'threshold':
        selectBelowThreshold();
        break;
      case 'clouded':
        selectCloudedSequence();
        break;
      case 'off-target':
        selectAstrometryIssues();
        break;
      case 'unsolved':
        selectUnsolved();
        break;
      case 'recommended':
        selectRecommended();
        break;
    }
  }, [
    selectAstrometryIssues,
    selectBelowThreshold,
    selectCloudedSequence,
    selectRecommended,
    selectUnsolved,
  ]);

  const selectedForReview = useMemo(() =>
    activeSequence?.images.filter(img => selectedImages.has(img.image_id)) ?? [],
  [activeSequence, selectedImages]);

  // Batch rejection is deliberately two-step: show the exact per-image
  // evidence/reason before changing scheduler grades. Each image's own reason
  // is written — the scheduler keeps rejectreason per image, so a mixed batch
  // must not collapse to one shared string.
  const confirmRejectSelected = useCallback(async () => {
    if (selectedImages.size === 0) return;
    const selected = activeSequence?.images.filter(img => selectedImages.has(img.image_id)) ?? [];
    const byReason = new Map<string, number[]>();
    for (const img of selected) {
      const reason = img.regrade_reason ?? 'Quality analysis';
      const ids = byReason.get(reason);
      if (ids) ids.push(img.image_id);
      else byReason.set(reason, [img.image_id]);
    }
    for (const [reason, ids] of byReason) {
      await grading.gradeBatch(ids, 'rejected', reason);
    }
    setSelectedImages(new Set());
    setShowRejectReview(false);
  }, [selectedImages, activeSequence, grading]);

  // Toggle individual image selection
  const toggleImage = useCallback((imageId: number) => {
    activeImageIdRef.current = imageId;
    setCurrentImageId(imageId);
    setSelectedImages(prev => {
      const next = new Set(prev);
      if (next.has(imageId)) {
        next.delete(imageId);
      } else {
        next.add(imageId);
      }
      return next;
    });
  }, [setCurrentImageId]);

  const moveImageCursor = useCallback((offset: -1 | 1) => {
    const currentImageId = activeImageIdRef.current;
    if (!activeSequence || currentImageId === null) return;
    const currentIndex = activeSequence.images.findIndex(
      image => image.image_id === currentImageId,
    );
    if (currentIndex < 0) return;
    const nextIndex = Math.max(
      0,
      Math.min(activeSequence.images.length - 1, currentIndex + offset),
    );
    if (nextIndex === currentIndex) return;
    const nextImageId = activeSequence.images[nextIndex].image_id;
    activeImageIdRef.current = nextImageId;
    setCurrentImageId(nextImageId);
  }, [activeSequence, setCurrentImageId]);

  const sequenceHotkeyOptions = useMemo(() => ({
    enabled: !!activeSequence && !isAnalyzing && !showRejectReview,
    preventDefault: true,
  }), [activeSequence, isAnalyzing, showRejectReview]);

  useHotkeys('left,up', () => moveImageCursor(-1), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('right,down', () => moveImageCursor(1), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('space', () => {
    const currentImageId = activeImageIdRef.current;
    if (currentImageId !== null) toggleImage(currentImageId);
  }, sequenceHotkeyOptions, [toggleImage]);

  const routeQuery = searchParams.toString();
  const openImage = useCallback((imageId: number) => {
    navigate(`/detail/${imageId}${routeQuery ? `?${routeQuery}` : ''}`);
  }, [navigate, routeQuery]);

  if (!projectId) {
    return (
      <div className="empty-state">
        Select a project to analyze image sequences
      </div>
    );
  }

  if (!targetId) {
    return (
      <div className="sequence-view">
        <div className="sequence-header">
          <h2>Sequence Analysis</h2>
          <p style={{ color: 'var(--color-text-muted)', marginTop: '0.5rem' }}>
            Select a target from the header to analyze its image sequences.
          </p>
        </div>
        {targets.length > 0 && (
          <div className="sequence-target-list">
            <h3>Available Targets</h3>
            <div className="target-cards">
              {targets.map(t => (
                <button
                  key={t.id}
                  className="target-card-btn"
                  onClick={() => {
                    // Preserve the existing query context (notably ?db=) and
                    // just set the chosen target, instead of rebuilding the URL
                    // from scratch and dropping the db slug.
                    const params = new URLSearchParams(searchParams);
                    params.set('project', String(projectId));
                    params.set('target', String(t.id));
                    navigate(`/sequence?${params.toString()}`);
                  }}
                >
                  <span className="target-card-name">{t.name}</span>
                  <span className="target-card-count">{t.image_count} images</span>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="sequence-view">
      {/* Controls bar */}
      <div className="sequence-controls sticky">
        <div className="sequence-primary-row">
          <h2>Sequence Analysis</h2>
          <div className="sequence-primary-actions">
            {availableFilters.length > 1 && (
              <div className="filter-input-group">
                <label htmlFor="sequence-filter">Filter:</label>
                <select
                  id="sequence-filter"
                  value={filterName || 'all'}
                  onChange={(e) => {
                    const val = e.target.value === 'all' ? undefined : e.target.value;
                    const params = new URLSearchParams(searchParams);
                    if (val) {
                      params.set('filterName', val);
                    } else {
                      params.delete('filterName');
                    }
                    navigate(`/sequence?${params.toString()}`);
                  }}
                >
                  <option value="all">All Filters</option>
                  {availableFilters.map(f => (
                    <option key={f} value={f}>{f}</option>
                  ))}
                </select>
              </div>
            )}
            <button
              type="button"
              className="header-button"
              onClick={() => analyze({ target_id: targetId, filter_name: filterName })}
              disabled={isAnalyzing}
            >
              {isAnalyzing ? 'Analyzing...' : 'Re-analyze'}
            </button>
            <button
              type="button"
              className="header-button"
              onClick={(event) => spatialScan.start(event.shiftKey || undefined)}
              disabled={spatialScan.isStarting || spatialScan.isRunning}
              title="Analyze FITS pixels for occlusion, photometry, plate solutions, target offset, tracking loss, and satellite trails. Shift-click to recompute all cached evidence from the current catalogs."
            >
              {spatialScan.isRunning
                ? `${spatialScan.status?.progress.stage === 'astrometry' ? 'Solving' : 'Scanning'} ${spatialScan.status?.progress.processed ?? 0}/${spatialScan.status?.progress.total ?? 0}...`
                : 'Scan Quality'}
            </button>
          </div>
          <div className="sequence-job-status" aria-live="polite">
            {spatialScan.isRunning && spatialScan.status?.progress.current_file && (
              <span
                className="spatial-scan-current"
                title={spatialScan.status.progress.current_file}
              >
                {spatialScan.status.progress.current_file}
              </span>
            )}
            {!spatialScan.isRunning
              && (spatialScan.status?.progress.errors ?? 0) > 0
              && spatialScan.status?.progress.last_error && (
                <span
                  className="spatial-scan-errors"
                  title={spatialScan.status.progress.last_error}
                >
                  {spatialScan.status.progress.errors} quality scan errors
                </span>
              )}
          </div>
          <div className="sequence-history-actions">
            <UndoRedoToolbar
              canUndo={grading.canUndo}
              canRedo={grading.canRedo}
              isProcessing={grading.isLoading}
              undoStackSize={grading.undoStackSize}
              redoStackSize={grading.redoStackSize}
              onUndo={grading.undo}
              onRedo={grading.redo}
              getLastAction={grading.getLastAction}
              getNextRedoAction={grading.getNextRedoAction}
              className="compact"
            />
          </div>
        </div>

        <div className="sequence-review-row">
          <div className="threshold-control">
            <label htmlFor="sequence-threshold">Threshold:</label>
            <input
              id="sequence-threshold"
              type="range"
              min="0"
              max="1"
              step="0.05"
              value={threshold}
              onChange={(e) => setThreshold(parseFloat(e.target.value))}
            />
            <span className="threshold-value">{threshold.toFixed(2)}</span>
          </div>
          <div className="selection-preset-control">
            <label htmlFor="sequence-select-preset">Select:</label>
            <select
              id="sequence-select-preset"
              value=""
              onChange={(event) => applySelectionPreset(event.target.value)}
            >
              <option value="" disabled>Choose images…</option>
              <option value="threshold">Below threshold</option>
              <option value="clouded">Clouded</option>
              <option value="off-target">Off target</option>
              <option value="unsolved">Unsolved</option>
              <option value="recommended">Recommended</option>
            </select>
          </div>
          <div className="sequence-selection-slot">
            {selectedImages.size > 0 && (
              <div className="selection-action-bar sequence-selection-bar" aria-label="Selected image actions">
                <span className="selection-count">{selectedImages.size} selected</span>
                <button
                  type="button"
                  className="action-button reject"
                  onClick={() => setShowRejectReview(true)}
                >
                  Review rejection
                </button>
                <button
                  type="button"
                  className="header-button"
                  onClick={() => setSelectedImages(new Set())}
                >
                  Clear
                </button>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Error state */}
      {analysisError && (
        <div className="sequence-error">
          Failed to analyze sequence: {(analysisError as Error).message}
        </div>
      )}

      {/* Loading state */}
      {isAnalyzing && (
        <div className="loading">Analyzing image sequences...</div>
      )}

      {/* Results */}
      {!isAnalyzing && sequences.length > 0 && (
        <>
          {/* Sequence tabs (if multiple) */}
          {sequences.length > 1 && (
            <div className="sequence-tabs">
              {sequences.map((seq, i) => (
                <button
                  key={i}
                  className={`sequence-tab ${i === activeSequenceIndex ? 'active' : ''}`}
                  onClick={() => setActiveSequenceIndex(i)}
                >
                  {formatSequenceLabel(seq)}
                </button>
              ))}
            </div>
          )}

          {activeSequence && (
            <>
              {/* Summary bar */}
              <div className="sequence-summary-bar">
                <div className="summary-stats">
                  <span className="summary-item excellent">{activeSequence.summary.excellent_count} excellent</span>
                  <span className="summary-item good">{activeSequence.summary.good_count} good</span>
                  <span className="summary-item fair">{activeSequence.summary.fair_count} fair</span>
                  <span className="summary-item poor">{activeSequence.summary.poor_count} poor</span>
                  <span className="summary-item bad">{activeSequence.summary.bad_count} bad</span>
                </div>
                <div className="summary-issues">
                  {activeSequence.summary.cloud_events_detected > 0 && (
                    <span className="issue-badge clouds">{activeSequence.summary.cloud_events_detected} cloud events</span>
                  )}
                  {activeSequence.summary.focus_drift_detected && (
                    <span className="issue-badge focus">Focus drift</span>
                  )}
                  {activeSequence.summary.tracking_issues_detected && (
                    <span className="issue-badge tracking">Tracking issues</span>
                  )}
                  {activeSequence.summary.out_of_target_count > 0 && (
                    <span className="issue-badge tracking">{activeSequence.summary.out_of_target_count} off target</span>
                  )}
                  {activeSequence.summary.plate_solve_failed_count > 0 && (
                    <span className="issue-badge clouds">{activeSequence.summary.plate_solve_failed_count} unsolved</span>
                  )}
                </div>
              </div>

              {/* Timeline chart */}
              <SequenceTimeline
                key={`${activeSequence.filter_name}-${activeSequence.session_start}-timeline`}
                images={activeSequence.images}
                threshold={threshold}
                currentImageId={activeImageId}
                selectedImages={selectedImages}
                onToggle={toggleImage}
              />

              <PointingScatter images={activeSequence.images} />

              {/* Image strip */}
              <SequenceStrip
                key={`${activeSequence.filter_name}-${activeSequence.session_start}-strip`}
                dbId={dbId!}
                images={activeSequence.images}
                imageMap={imageMap}
                projectId={projectId!}
                targetId={activeSequence.target_id}
                targetName={activeSequence.target_name}
                filterName={activeSequence.filter_name}
                currentImageId={activeImageId}
                selectedImages={selectedImages}
                threshold={threshold}
                onToggle={toggleImage}
                onOpen={openImage}
              />
            </>
          )}
        </>
      )}

      {/* No results */}
      {!isAnalyzing && !analysisError && sequences.length === 0 && analysisData && (
        <div className="empty-state">
          No sequences found for this target. Make sure images have been captured.
        </div>
      )}

      <Dialog
        open={showRejectReview}
        title={`Review ${selectedForReview.length} recommended rejection${selectedForReview.length === 1 ? '' : 's'}`}
        onClose={() => setShowRejectReview(false)}
        className="reject-review-dialog"
        footer={(
          <>
            <button type="button" className="header-button" onClick={() => setShowRejectReview(false)}>
              Cancel
            </button>
            <button
              type="button"
              className="action-button reject"
              onClick={confirmRejectSelected}
              disabled={grading.isLoading}
            >
              Confirm rejection ({selectedForReview.length})
            </button>
          </>
        )}
      >
        <p className="dialog-intro">
          Existing rejections remain unchanged. Review the quality score and evidence before writing these grades.
        </p>
        <div className="reject-review-list">
          {selectedForReview.map(image => (
            <div key={image.image_id} className="reject-review-item">
              <strong>Image {image.image_id}</strong> · score {image.quality_score.toFixed(2)}
              <div className={image.regrade_reason ? 'reject-review-reason' : 'reject-review-warning'}>
                {image.regrade_reason ?? 'Manually selected; no automatic rejection recommendation'}
              </div>
              {image.details && <div className="reject-review-details">{image.details}</div>}
            </div>
          ))}
        </div>
      </Dialog>
    </div>
  );
}

const PointingScatter = memo(function PointingScatter({ images }: { images: ImageQualityResult[] }) {
  const points = images.flatMap(image => {
    const east = image.pointing?.east_offset_arcsec;
    const north = image.pointing?.north_offset_arcsec;
    return east != null && north != null
      ? [{ image, east, north }]
      : [];
  });
  if (points.length < 2) return null;
  const expectedTarget = points.some(point => point.image.pointing?.expected_target);

  const extent = Math.max(
    30,
    ...points.flatMap(point => [Math.abs(point.east), Math.abs(point.north)])
  ) * 1.1;
  const size = 180;
  const center = size / 2;
  const project = (value: number) => center + (value / extent) * (center - 15);

  return (
    <div className="pointing-scatter" style={{ display: 'flex', gap: '1rem', alignItems: 'center', margin: '0.75rem 0' }}>
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} role="img" aria-label="Solved pointing offsets">
        <rect x="0" y="0" width={size} height={size} fill="var(--color-bg-secondary)" rx="6" />
        <line x1={center} y1="10" x2={center} y2={size - 10} stroke="var(--color-text-muted)" opacity="0.45" />
        <line x1="10" y1={center} x2={size - 10} y2={center} stroke="var(--color-text-muted)" opacity="0.45" />
        {points.map(({ image, east, north }) => (
          <circle
            key={image.image_id}
            cx={project(east)}
            cy={project(-north)}
            r={(image.flags ?? []).some(flag => flag === 'off_target' || flag === 'pointing_jump' || flag === 'pointing_drift') ? 5 : 3}
            fill={qualityColor(image.quality_score)}
          >
            <title>Image {image.image_id}: E {east.toFixed(0)}″, N {north.toFixed(0)}″</title>
          </circle>
        ))}
        <text x={size - 12} y={center - 4} textAnchor="end" fontSize="9" fill="var(--color-text-muted)">E</text>
        <text x={center + 4} y="12" fontSize="9" fill="var(--color-text-muted)">N</text>
      </svg>
      <div style={{ color: 'var(--color-text-muted)', fontSize: '0.8rem' }}>
        <strong style={{ color: 'var(--color-text-primary)' }}>Solved pointing</strong><br />
        {expectedTarget ? 'Target' : 'First solved center'} is the crosshair. Range ±{extent.toFixed(0)}″.<br />
        Large points are off-target, jumps, or drift.
      </div>
    </div>
  );
});

// Timeline visualization component
const SequenceTimeline = memo(function SequenceTimeline({
  images,
  threshold,
  currentImageId,
  selectedImages,
  onToggle,
}: {
  images: ImageQualityResult[];
  threshold: number;
  currentImageId: number | null;
  selectedImages: Set<number>;
  onToggle: (id: number) => void;
}) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const [viewportWidth, setViewportWidth] = useState(0);

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;

    const updateWidth = () => {
      const next = Math.floor(viewport.clientWidth);
      setViewportWidth(current => current === next ? current : next);
    };
    updateWidth();

    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(updateWidth);
    observer.observe(viewport);
    return () => observer.disconnect();
  }, []);

  if (images.length === 0) return null;

  const chartWidth = Math.max(400, viewportWidth);
  const chartHeight = 120;
  const padding = { top: 10, right: 10, bottom: 20, left: 30 };
  const innerWidth = chartWidth - padding.left - padding.right;
  const innerHeight = chartHeight - padding.top - padding.bottom;

  const barStep = innerWidth / images.length;
  const barWidth = Math.max(0.75, Math.min(10, barStep * 0.8));

  return (
    <div className="sequence-timeline">
      <div ref={viewportRef} className="sequence-timeline-scroll">
        <svg
          width={chartWidth}
          height={chartHeight}
          viewBox={`0 0 ${chartWidth} ${chartHeight}`}
          role="img"
          aria-label="Sequence quality scores"
        >
        {/* Threshold line */}
        <line
          x1={padding.left}
          y1={padding.top + innerHeight * (1 - threshold)}
          x2={chartWidth - padding.right}
          y2={padding.top + innerHeight * (1 - threshold)}
          stroke="var(--color-warning)"
          strokeWidth="1"
          strokeDasharray="4,4"
          opacity="0.6"
        />

        {/* Y-axis labels */}
        <text x={padding.left - 4} y={padding.top + 4} fontSize="9" fill="var(--color-text-muted)" textAnchor="end">1.0</text>
        <text x={padding.left - 4} y={padding.top + innerHeight / 2 + 3} fontSize="9" fill="var(--color-text-muted)" textAnchor="end">0.5</text>
        <text x={padding.left - 4} y={padding.top + innerHeight + 3} fontSize="9" fill="var(--color-text-muted)" textAnchor="end">0.0</text>

        {/* Bars */}
        {images.map((img, i) => {
          const x = padding.left + i * barStep + (barStep - barWidth) / 2;
          const barHeight = img.quality_score * innerHeight;
          const y = padding.top + innerHeight - barHeight;
          const isSelected = selectedImages.has(img.image_id);
          const isCurrent = currentImageId === img.image_id;

          return (
            <rect
              key={img.image_id}
              x={x}
              y={y}
              width={barWidth}
              height={Math.max(1, barHeight)}
              fill={qualityColor(img.quality_score)}
              opacity={isSelected || isCurrent ? 1 : 0.7}
              stroke={isCurrent
                ? 'var(--color-primary)'
                : isSelected ? 'var(--color-warning)' : 'none'}
              strokeWidth={isCurrent ? 2 : isSelected ? 1 : 0}
              style={{ cursor: 'pointer' }}
              onClick={() => onToggle(img.image_id)}
            >
              <title>Score: {img.quality_score.toFixed(2)}{img.category ? ` (${formatCategory(img.category)})` : ''}</title>
            </rect>
          );
        })}
        </svg>
      </div>
    </div>
  );
});

const SequenceStrip = memo(function SequenceStrip({
  dbId,
  images,
  imageMap,
  projectId,
  targetId,
  targetName,
  filterName,
  currentImageId,
  selectedImages,
  threshold,
  onToggle,
  onOpen,
}: {
  dbId: string;
  images: ImageQualityResult[];
  imageMap: ReadonlyMap<number, Image>;
  projectId: number;
  targetId: number;
  targetName: string;
  filterName: string;
  currentImageId: number | null;
  selectedImages: Set<number>;
  threshold: number;
  onToggle: (id: number) => void;
  onOpen: (id: number) => void;
}) {
  const stripRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (currentImageId === null) return;
    requestAnimationFrame(() => {
      const currentCard = stripRef.current?.querySelector<HTMLElement>(
        '.sequence-image-card.current-selection',
      );
      currentCard?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
    });
  }, [currentImageId]);

  return (
    <div ref={stripRef} className="filter-images sequence-strip">
      {images.map(quality => {
        const image = imageMap.get(quality.image_id) ?? {
          id: quality.image_id,
          project_id: projectId,
          project_name: '',
          project_display_name: '',
          target_id: targetId,
          target_name: targetName,
          acquired_date: null,
          filter_name: filterName,
          grading_status: 0,
          reject_reason: null,
          metadata: {},
          filesystem_path: null,
        };
        const belowThreshold = quality.quality_score < threshold;
        return (
          <ImageCard
            key={quality.image_id}
            dbId={dbId}
            image={image}
            quality={quality}
            isSelected={selectedImages.has(quality.image_id)}
            onClick={() => onToggle(quality.image_id)}
            onDoubleClick={() => onOpen(quality.image_id)}
            lazyPreview
            selectionEffects={false}
            className={`sequence-image-card${
              currentImageId === quality.image_id ? ' current-selection' : ''
            }${belowThreshold ? ' below-threshold' : ''}`}
          />
        );
      })}
    </div>
  );
});
