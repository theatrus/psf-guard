import { useState, useMemo, useCallback, useEffect } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useSequenceAnalysis } from '../hooks/useSequenceAnalysis';
import { useSpatialScan } from '../hooks/useSpatialScan';
import { useGrading } from '../hooks/useGrading';
import { useDbProjectTarget } from '../hooks/useUrlState';
import UndoRedoToolbar from './UndoRedoToolbar';
import PreviewImage from './PreviewImage';
import type { ScoredSequence, ImageQualityResult } from '../api/types';

function formatCategory(category: string): string {
  return category
    .split('_')
    .map(w => w.charAt(0).toUpperCase() + w.slice(1))
    .join(' ');
}

function qualityColor(score: number): string {
  if (score >= 0.7) return 'var(--color-success)';
  if (score >= 0.5) return 'var(--color-warning)';
  return 'var(--color-error)';
}

function qualityLabel(score: number): string {
  if (score >= 0.85) return 'Excellent';
  if (score >= 0.7) return 'Good';
  if (score >= 0.5) return 'Fair';
  if (score >= 0.3) return 'Poor';
  return 'Bad';
}

export default function SequenceView() {
  const { dbId, projectId, targetId } = useDbProjectTarget();
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
    setSelectedImages(prev => {
      const next = new Set(prev);
      if (next.has(imageId)) {
        next.delete(imageId);
      } else {
        next.add(imageId);
      }
      return next;
    });
  }, []);

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
        <div className="sequence-controls-row">
          <div className="sequence-controls-left">
            <h2>Sequence Analysis</h2>
            {availableFilters.length > 1 && (
              <div className="filter-input-group">
                <label>Filter:</label>
                <select
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
              className="header-button"
              onClick={() => analyze({ target_id: targetId, filter_name: filterName })}
              disabled={isAnalyzing}
            >
              {isAnalyzing ? 'Analyzing...' : 'Re-analyze'}
            </button>
            <button
              className="header-button"
              onClick={() => spatialScan.start(undefined)}
              disabled={spatialScan.isStarting || spatialScan.isRunning}
              title="Analyze FITS pixels for occlusion, photometry, plate solutions, target offset, and tracking loss. Runs in the background and reuses cached results."
            >
              {spatialScan.isRunning
                ? `${spatialScan.status?.progress.stage === 'astrometry' ? 'Solving' : 'Scanning'} ${spatialScan.status?.progress.processed ?? 0}/${spatialScan.status?.progress.total ?? 0}...`
                : 'Scan Quality'}
            </button>
            {spatialScan.isRunning && spatialScan.status?.progress.current_file && (
              <span
                className="spatial-scan-current"
                style={{ color: 'var(--color-text-muted)', fontSize: '0.8rem' }}
                title={spatialScan.status.progress.current_file}
              >
                {spatialScan.status.progress.current_file}
              </span>
            )}
            {!spatialScan.isRunning &&
              (spatialScan.status?.progress.errors ?? 0) > 0 &&
              spatialScan.status?.progress.last_error && (
                <span
                  className="spatial-scan-errors"
                  style={{ color: 'var(--color-warning)', fontSize: '0.8rem' }}
                  title={spatialScan.status.progress.last_error}
                >
                  {spatialScan.status.progress.errors} quality scan errors
                </span>
              )}
          </div>

          <div className="sequence-controls-right">
            <div className="threshold-control">
              <label>Threshold:</label>
              <input
                type="range"
                min="0"
                max="1"
                step="0.05"
                value={threshold}
                onChange={(e) => setThreshold(parseFloat(e.target.value))}
              />
              <span className="threshold-value">{threshold.toFixed(2)}</span>
            </div>
            <button className="header-button" onClick={selectBelowThreshold}>
              Select Below Threshold
            </button>
            <button className="header-button" onClick={selectCloudedSequence}>
              Select Clouded
            </button>
            <button className="header-button" onClick={selectAstrometryIssues}>
              Select Off Target
            </button>
            <button className="header-button" onClick={selectUnsolved}>
              Select Unsolved
            </button>
            <button className="header-button" onClick={selectRecommended}>
              Select Recommended
            </button>
            {selectedImages.size > 0 && (
              <>
                <button
                  className="action-button reject"
                  style={{ padding: '0.4rem 0.8rem', fontSize: '0.85rem' }}
                  onClick={() => setShowRejectReview(true)}
                >
                  Reject Selected ({selectedImages.size})
                </button>
                <button
                  className="header-button"
                  onClick={() => setSelectedImages(new Set())}
                >
                  Clear
                </button>
              </>
            )}
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
                  {seq.filter_name} ({seq.image_count})
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
                images={activeSequence.images}
                threshold={threshold}
                selectedImages={selectedImages}
                onToggle={toggleImage}
              />

              <PointingScatter images={activeSequence.images} />

              {/* Image strip */}
              <div className="sequence-strip">
                {activeSequence.images.map((qr) => {
                  const img = imageMap.get(qr.image_id);
                  const isSelected = selectedImages.has(qr.image_id);
                  return (
                    <SequenceImageCard
                      key={qr.image_id}
                      dbId={dbId!}
                      quality={qr}
                      image={img}
                      isSelected={isSelected}
                      isBelowThreshold={qr.quality_score < threshold}
                      onClick={() => toggleImage(qr.image_id)}
                      onDoubleClick={() => {
                        const params = searchParams.toString();
                        navigate(`/detail/${qr.image_id}?${params}`);
                      }}
                    />
                  );
                })}
              </div>
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

      {showRejectReview && (
        <div
          role="presentation"
          onClick={() => setShowRejectReview(false)}
          style={{
            position: 'fixed', inset: 0, zIndex: 1000,
            background: 'rgba(0, 0, 0, 0.65)', display: 'grid', placeItems: 'center',
            padding: '1rem',
          }}
        >
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="reject-review-title"
            onClick={event => event.stopPropagation()}
            style={{
              width: 'min(760px, 100%)', maxHeight: '80vh', overflow: 'auto',
              background: 'var(--color-surface)', color: 'var(--color-text)',
              border: '1px solid var(--color-border)', borderRadius: '8px', padding: '1rem',
            }}
          >
            <h3 id="reject-review-title">Review {selectedForReview.length} recommended rejection{selectedForReview.length === 1 ? '' : 's'}</h3>
            <p style={{ color: 'var(--color-text-muted)' }}>
              Existing rejections remain unchanged. Review the quality score and evidence before writing these grades.
            </p>
            <div style={{ display: 'grid', gap: '0.5rem', margin: '1rem 0' }}>
              {selectedForReview.map(image => (
                <div key={image.image_id} style={{ borderTop: '1px solid var(--color-border)', paddingTop: '0.5rem' }}>
                  <strong>Image {image.image_id}</strong> · score {image.quality_score.toFixed(2)}
                  <div style={{ color: image.regrade_reason ? 'var(--color-error)' : 'var(--color-warning)', fontSize: '0.85rem' }}>
                    {image.regrade_reason ?? 'Manually selected; no automatic rejection recommendation'}
                  </div>
                  {image.details && <div style={{ color: 'var(--color-text-muted)', fontSize: '0.8rem' }}>{image.details}</div>}
                </div>
              ))}
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: '0.5rem' }}>
              <button className="header-button" onClick={() => setShowRejectReview(false)}>Cancel</button>
              <button className="action-button reject" onClick={confirmRejectSelected} disabled={grading.isLoading}>
                Confirm rejection ({selectedForReview.length})
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function PointingScatter({ images }: { images: ImageQualityResult[] }) {
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
        <rect x="0" y="0" width={size} height={size} fill="var(--color-surface)" rx="6" />
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
        <strong style={{ color: 'var(--color-text)' }}>Solved pointing</strong><br />
        {expectedTarget ? 'Target' : 'First solved center'} is the crosshair. Range ±{extent.toFixed(0)}″.<br />
        Large points are off-target, jumps, or drift.
      </div>
    </div>
  );
}

// Timeline visualization component
function SequenceTimeline({
  images,
  threshold,
  selectedImages,
  onToggle,
}: {
  images: ImageQualityResult[];
  threshold: number;
  selectedImages: Set<number>;
  onToggle: (id: number) => void;
}) {
  if (images.length === 0) return null;

  const chartWidth = Math.max(images.length * 12, 400);
  const chartHeight = 120;
  const padding = { top: 10, right: 10, bottom: 20, left: 30 };
  const innerWidth = chartWidth - padding.left - padding.right;
  const innerHeight = chartHeight - padding.top - padding.bottom;

  const barWidth = Math.max(2, Math.min(10, (innerWidth / images.length) - 1));

  return (
    <div className="sequence-timeline">
      <svg
        width="100%"
        height={chartHeight}
        viewBox={`0 0 ${chartWidth} ${chartHeight}`}
        preserveAspectRatio="none"
        style={{ display: 'block' }}
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
          const x = padding.left + (i / images.length) * innerWidth;
          const barHeight = img.quality_score * innerHeight;
          const y = padding.top + innerHeight - barHeight;
          const isSelected = selectedImages.has(img.image_id);

          return (
            <rect
              key={img.image_id}
              x={x}
              y={y}
              width={barWidth}
              height={Math.max(1, barHeight)}
              fill={qualityColor(img.quality_score)}
              opacity={isSelected ? 1 : 0.7}
              stroke={isSelected ? 'var(--color-primary)' : 'none'}
              strokeWidth={isSelected ? 1 : 0}
              style={{ cursor: 'pointer' }}
              onClick={() => onToggle(img.image_id)}
            >
              <title>Score: {img.quality_score.toFixed(2)}{img.category ? ` (${formatCategory(img.category)})` : ''}</title>
            </rect>
          );
        })}
      </svg>
    </div>
  );
}

// Individual image card in the sequence strip
function SequenceImageCard({
  dbId,
  quality,
  image,
  isSelected,
  isBelowThreshold,
  onClick,
  onDoubleClick,
}: {
  dbId: string;
  quality: ImageQualityResult;
  image?: { id: number; target_name: string; filter_name: string | null; acquired_date: number | null; grading_status: number };
  isSelected: boolean;
  isBelowThreshold: boolean;
  onClick: () => void;
  onDoubleClick: () => void;
}) {
  const formatDate = (timestamp: number | null | undefined) => {
    if (!timestamp) return '';
    return new Date(timestamp * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  };

  return (
    <div
      className={`sequence-image-card ${isSelected ? 'selected' : ''} ${isBelowThreshold ? 'below-threshold' : ''}`}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
    >
      <div className="sequence-image-preview">
        <PreviewImage
          dbId={dbId}
          src={apiClient.getPreviewUrl(dbId, quality.image_id, { size: 'screen' })}
          descriptor={{ imageId: quality.image_id, kind: 'preview', size: 'screen' }}
          alt={`Image ${quality.image_id}`}
          loading="lazy"
        />
        {/* Quality badge */}
        <div
          className="quality-badge"
          style={{ backgroundColor: qualityColor(quality.quality_score) }}
          title={`Quality: ${quality.quality_score.toFixed(2)} - ${qualityLabel(quality.quality_score)}`}
        >
          {(quality.quality_score * 100).toFixed(0)}
        </div>
        {/* Category label */}
        {quality.category && (
          <div className="category-label">
            {formatCategory(quality.category)}
          </div>
        )}
      </div>
      <div className="sequence-image-info">
        <span className="sequence-image-time">{formatDate(image?.acquired_date)}</span>
        {quality.normalized_metrics.spatial_coverage != null &&
          quality.normalized_metrics.spatial_coverage < 0.9 && (
            <span
              className="sequence-image-coverage"
              style={{ color: 'var(--color-warning)', fontSize: '0.75rem' }}
              title="Spatial star coverage from the occlusion scan (1.0 = stars across the whole frame)"
            >
              coverage {quality.normalized_metrics.spatial_coverage.toFixed(2)}
            </span>
          )}
        {quality.pointing?.field_fraction_offset != null && (
          <span
            className="sequence-image-pointing"
            style={{ color: quality.regrade_reason ? 'var(--color-error)' : 'var(--color-text-muted)', fontSize: '0.75rem' }}
            title={`Solved target offset: ${quality.pointing.separation_arcsec?.toFixed(0) ?? '?'} arcsec`}
          >
            offset {(quality.pointing.field_fraction_offset * 100).toFixed(0)}% field
          </span>
        )}
        {quality.pointing?.solve_failed && (
          <span
            className="sequence-image-pointing"
            style={{ color: 'var(--color-warning)', fontSize: '0.75rem' }}
            title={quality.pointing.error || (quality.pointing.image_quality_evidence
              ? 'Pixels did not match a field'
              : 'Plate solver could not make a quality determination')}
          >
            {quality.pointing.image_quality_evidence ? 'unsolved' : 'solve unavailable'}
          </span>
        )}
        {quality.details && (
          <span className="sequence-image-details" title={quality.details}>
            {quality.details}
          </span>
        )}
      </div>
    </div>
  );
}
