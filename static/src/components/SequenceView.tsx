import {
  memo,
  useState,
  useMemo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
} from 'react';
import type { RefObject } from 'react';
import { useLocation, useSearchParams, useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import { useSequenceAnalysis } from '../hooks/useSequenceAnalysis';
import { useSpatialScan } from '../hooks/useSpatialScan';
import ScoringPenaltyControl from './ScoringPenaltyControl';
import { useGrading } from '../hooks/useGrading';
import { useDbProjectTarget, useGridState } from '../hooks/useUrlState';
import UndoRedoToolbar from './UndoRedoToolbar';
import ImageCard from './ImageCard';
import Dialog from './Dialog';
import ThumbnailSizeControl from './ThumbnailSizeControl';
import { QualityScanButton } from './QualityScanControl';
import type { Image, ScoredSequence, ImageQualityResult } from '../api/types';
import {
  imageDetailNavigationState,
  imageDetailPath,
  sequenceReturnPositionFromState,
} from '../utils/imageDetailRoutes';
import {
  findGridNavigationIndex,
  type GridNavigationDirection,
} from '../utils/gridNavigation';
import { thumbnailGridColumns } from '../utils/thumbnailSizing';
import { formatCategory } from '../utils/issueCategory';
import SecondaryScoreToggle from './SecondaryScoreToggle';
import {
  type BasisScores,
  qualityScoreDescription,
  type QualityScoreScope,
} from '../utils/qualityScore';

interface SequenceChoice {
  key: string;
  label: string;
  sequence: ScoredSequence;
  scoreScope: QualityScoreScope;
  unavailableImageCount: number;
}

function formatSequenceLabel(sequence: ScoredSequence): string {
  if (!sequence.session_start) return `${sequence.filter_name} (${sequence.image_count})`;
  const start = new Date(sequence.session_start * 1000);
  const when = start.toLocaleString([], {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
  return `${sequence.filter_name} · ${when} (${sequence.image_count})`;
}

function sequenceKey(sequence: ScoredSequence): string {
  return [
    sequence.target_id,
    sequence.filter_name,
    sequence.session_start ?? 'unknown',
    sequence.images[0]?.image_id ?? 'empty',
  ].join('-');
}

function qualityColor(score: number): string {
  if (score >= 0.7) return 'var(--color-success)';
  if (score >= 0.5) return 'var(--color-warning)';
  return 'var(--color-error)';
}

export default function SequenceView() {
  const { dbId, projectId, targetId, setTargetId } = useDbProjectTarget();
  const {
    imageSize,
    currentImageId: urlCurrentImageId,
    selectedImages,
    setImageSize,
    setCurrentImageId,
    setSelectedImages,
  } = useGridState(150);
  const location = useLocation();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const grading = useGrading(dbId!);
  const { analyze, data: analysisData, isLoading: isAnalyzing, error: analysisError } = useSequenceAnalysis(dbId);

  const filterName = searchParams.get('filterName') || undefined;
  const requestedScoreScope = searchParams.get('scoreScope');
  const [threshold, setThreshold] = useState(0.5);
  const [showRejectReview, setShowRejectReview] = useState(false);
  const selectionAnchorIdRef = useRef<number | null>(null);
  const selectionBaseIdsRef = useRef<Set<number>>(new Set());
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

  // Preserve the Grid cursor when it identifies a target. With no cursor,
  // skip the target picker when the project has only one choice.
  const entrySelectionRef = useRef<Set<number> | null>(new Set(selectedImages));
  const inferredSelectionRef = useRef<Set<number> | null>(null);
  useEffect(() => {
    if (!projectId || targetId) return;
    const currentTargetId = urlCurrentImageId === null
      ? null
      : imageMap.get(urlCurrentImageId)?.target_id ?? null;
    const onlyTargetId = targets.length === 1 ? targets[0].id : null;
    const inferredTargetId = currentTargetId ?? onlyTargetId;
    if (inferredTargetId !== null) {
      const inferredSelection = new Set(
        Array.from(entrySelectionRef.current ?? []).filter(imageId => {
          const image = imageMap.get(imageId);
          return image?.target_id === inferredTargetId || (
            image === undefined && onlyTargetId === inferredTargetId
          );
        }),
      );
      entrySelectionRef.current = inferredSelection;
      inferredSelectionRef.current = inferredSelection;
      setTargetId(inferredTargetId);
    }
  }, [
    imageMap,
    projectId,
    setTargetId,
    targetId,
    targets,
    urlCurrentImageId,
  ]);

  // A selection may span sessions for one target, but never survives a scope
  // change where its quality evidence is no longer loaded.
  const selectionScope = `${dbId ?? ''}:${projectId ?? ''}:${targetId ?? ''}:${filterName ?? ''}`;
  const selectionScopeRef = useRef(selectionScope);
  useEffect(() => {
    if (selectionScopeRef.current !== selectionScope) {
      selectionScopeRef.current = selectionScope;
      const inferredSelection = inferredSelectionRef.current;
      inferredSelectionRef.current = null;
      if (inferredSelection === null) entrySelectionRef.current = null;
      selectionAnchorIdRef.current = null;
      selectionBaseIdsRef.current = new Set();
      setSelectedImages(inferredSelection ?? new Set());
    }
  }, [selectionScope, setSelectedImages]);

  // Auto-analyze when target is selected
  useEffect(() => {
    if (targetId) {
      analyze({ target_id: targetId, filter_name: filterName });
    }
  }, [targetId, filterName, analyze]);

  const sequences = useMemo(() => analysisData?.sequences ?? [], [analysisData?.sequences]);
  const rollups = useMemo(
    () => analysisData?.target_filter_rollups ?? [],
    [analysisData?.target_filter_rollups],
  );
  const sequenceChoices = useMemo(() => {
    const choices: SequenceChoice[] = [];
    const sessionsByFilter = new Map<string, ScoredSequence[]>();
    const rollupsByFilter = new Map(
      rollups.map(rollup => [rollup.filter_name, rollup]),
    );
    sequences.forEach(sequence => {
      const sessions = sessionsByFilter.get(sequence.filter_name);
      if (sessions) sessions.push(sequence);
      else sessionsByFilter.set(sequence.filter_name, [sequence]);
    });
    const sessionEvidence = new Map(
      sequences.flatMap(sequence => sequence.images).map(image => [image.image_id, image]),
    );

    // Keep the server's chronological session order so the tabs read left to
    // right in capture order. Each filter's all-session view stays first.
    sessionsByFilter.forEach((sessions, filter) => {
      const rollup = rollupsByFilter.get(filter);
      if (rollup && sessions.length > 1) {
        const images = rollup.images.flatMap(score => {
          const session = sessionEvidence.get(score.image_id);
          return session ? [{
            ...session,
            quality_score: score.quality_score,
            normalized_metrics: score.normalized_metrics,
            details: score.details,
          }] : [];
        });
        const sequence: ScoredSequence = {
          target_id: rollup.target_id,
          target_name: rollup.target_name,
          filter_name: rollup.filter_name,
          session_start: rollup.session_start,
          session_end: rollup.session_end,
          image_count: images.length,
          reference_values: {},
          images,
          summary: rollup.summary,
        };
        choices.push({
          key: `target-filter:${filter}`,
          label: `${filter} · All sessions (${rollup.image_count})`,
          sequence,
          scoreScope: 'target_filter',
          unavailableImageCount: rollup.unavailable_image_count,
        });
      }
      sessions.forEach(sequence => choices.push({
        key: `session:${sequenceKey(sequence)}`,
        label: formatSequenceLabel(sequence),
        sequence,
        scoreScope: 'capture_sequence',
        unavailableImageCount: 0,
      }));
    });
    return choices;
  }, [rollups, sequences]);

  // Grid selection shares URL state with Sequence. Normalize that initial
  // selection once the active target's analysis has loaded. Later Sequence
  // selections must remain untouched.
  useEffect(() => {
    const entrySelection = entrySelectionRef.current;
    if (!targetId || !analysisData || isAnalyzing || entrySelection === null) return;

    entrySelectionRef.current = null;
    const sequenceImageIds = new Set(
      sequences.flatMap(sequence => sequence.images.map(image => image.image_id)),
    );
    const normalizedSelection = new Set(
      Array.from(entrySelection).filter(imageId => sequenceImageIds.has(imageId)),
    );
    const unchanged = normalizedSelection.size === selectedImages.size
      && Array.from(normalizedSelection).every(imageId => selectedImages.has(imageId));
    if (!unchanged) setSelectedImages(normalizedSelection);
  }, [analysisData, isAnalyzing, selectedImages, sequences, setSelectedImages, targetId]);
  const activeChoice = useMemo(() => {
    if (requestedScoreScope?.startsWith('target-filter:')) {
      const requestedFilter = requestedScoreScope.slice('target-filter:'.length);
      const rollup = sequenceChoices.find(choice =>
        choice.scoreScope === 'target_filter'
        && choice.sequence.filter_name === requestedFilter
      );
      if (rollup) return rollup;
    }
    if (urlCurrentImageId !== null) {
      const currentSession = sequenceChoices.find(choice =>
        choice.scoreScope === 'capture_sequence'
        && choice.sequence.images.some(image => image.image_id === urlCurrentImageId)
      );
      if (currentSession) return currentSession;
    }

    return sequenceChoices.find(choice => choice.scoreScope === 'target_filter')
      ?? sequenceChoices[0];
  }, [requestedScoreScope, sequenceChoices, urlCurrentImageId]);
  const activeSequence = activeChoice?.sequence;
  const activeScoreScope: QualityScoreScope = activeChoice?.scoreScope ?? 'capture_sequence';
  // Every basis score per frame, for the always-on chips: the session
  // score, and the all-sessions score for filters with several sessions.
  // Built for every filter in view so the chips never depend on which
  // session strip is selected.
  const basisScoresByImage = useMemo(() => {
    const map = new Map<number, BasisScores>();
    const sessionsPerFilter = new Map<string, number>();
    for (const sequence of sequences) {
      sessionsPerFilter.set(
        sequence.filter_name,
        (sessionsPerFilter.get(sequence.filter_name) ?? 0) + 1,
      );
      for (const image of sequence.images) {
        map.set(image.image_id, { night: image.quality_score });
      }
    }
    for (const rollup of rollups) {
      if ((sessionsPerFilter.get(rollup.filter_name) ?? 0) <= 1) continue;
      for (const score of rollup.images) {
        const entry = map.get(score.image_id);
        if (entry) entry.all = score.quality_score;
      }
    }
    return map;
  }, [rollups, sequences]);
  const unavailableImageCount = activeChoice?.unavailableImageCount ?? 0;
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
  const sequenceStripRef = useRef<HTMLDivElement>(null);
  const restoredScrollRef = useRef(false);

  useEffect(() => {
    if (selectionAnchorIdRef.current === null && activeImageId !== null) {
      selectionAnchorIdRef.current = activeImageId;
      const activeImageIds = new Set(
        activeSequence?.images.map(image => image.image_id) ?? [],
      );
      selectionBaseIdsRef.current = new Set(
        Array.from(selectedImages).filter(imageId => !activeImageIds.has(imageId)),
      );
    }
  }, [activeImageId, activeSequence, selectedImages]);

  useLayoutEffect(() => {
    const position = sequenceReturnPositionFromState(location.state);
    if (restoredScrollRef.current || !position || isAnalyzing || !activeSequence) {
      return;
    }

    const scroller = document.querySelector<HTMLElement>('.app-main');
    if (!scroller) return;
    restoredScrollRef.current = true;

    const restorePosition = () => {
      const anchor = document.querySelector<HTMLElement>(
        `.sequence-image-card[data-card-image-id="${position.imageId}"]`,
      );
      if (!anchor) {
        scroller.scrollTop = position.scrollTop;
        return;
      }
      const currentOffset =
        anchor.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      scroller.scrollTop += currentOffset - position.offsetTop;
    };

    restorePosition();
    let secondFrame = 0;
    const firstFrame = requestAnimationFrame(() => {
      restorePosition();
      secondFrame = requestAnimationFrame(restorePosition);
    });
    return () => {
      cancelAnimationFrame(firstFrame);
      if (secondFrame) cancelAnimationFrame(secondFrame);
    };
  }, [activeSequence, isAnalyzing, location.state]);

  useEffect(() => {
    if (activeImageId !== null && activeImageId !== urlCurrentImageId) {
      if (activeScoreScope === 'target_filter' && !requestedScoreScope) {
        const params = new URLSearchParams(searchParams);
        params.set('current', String(activeImageId));
        params.set('scoreScope', `target-filter:${activeSequence?.filter_name ?? ''}`);
        params.delete('groupIndex');
        params.delete('imageIndex');
        navigate(`/sequence?${params.toString()}`, { replace: true });
      } else {
        setCurrentImageId(activeImageId);
      }
    }
  }, [
    activeImageId,
    activeScoreScope,
    activeSequence?.filter_name,
    navigate,
    requestedScoreScope,
    searchParams,
    setCurrentImageId,
    urlCurrentImageId,
  ]);

  const replaceSelectedImages = useCallback((ids: Set<number>) => {
    const anchorId = activeImageIdRef.current;
    const base = new Set(ids);
    if (anchorId !== null) base.delete(anchorId);
    selectionAnchorIdRef.current = anchorId;
    selectionBaseIdsRef.current = base;
    setSelectedImages(ids);
  }, [setSelectedImages]);

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
    replaceSelectedImages(ids);
  }, [activeSequence, replaceSelectedImages, threshold]);

  // Select frames for which a detector named cloud or obstruction evidence.
  // A low relative score alone must not masquerade as a diagnosis.
  const selectCloudedSequence = useCallback(() => {
    if (!activeSequence) return;
    replaceSelectedImages(new Set(
      activeSequence.images
        .filter(image => image.category === 'likely_clouds'
          || image.category === 'possible_obstruction')
        .map(image => image.image_id),
    ));
  }, [activeSequence, replaceSelectedImages]);

  const selectAstrometryIssues = useCallback(() => {
    if (!activeSequence) return;
    replaceSelectedImages(new Set(
      activeSequence.images
        .filter(img => (img.flags ?? []).some(flag =>
          flag === 'off_target' || flag === 'pointing_jump' || flag === 'pointing_drift'))
        .map(img => img.image_id)
    ));
  }, [activeSequence, replaceSelectedImages]);

  const selectUnsolved = useCallback(() => {
    if (!activeSequence) return;
    replaceSelectedImages(new Set(
      activeSequence.images
        .filter(img => img.pointing?.solve_failed && img.pointing.image_quality_evidence)
        .map(img => img.image_id)
    ));
  }, [activeSequence, replaceSelectedImages]);

  const selectRecommended = useCallback(() => {
    if (!activeSequence) return;
    replaceSelectedImages(new Set(
      activeSequence.images
        .filter(img => !!img.regrade_reason)
        .map(img => img.image_id)
    ));
  }, [activeSequence, replaceSelectedImages]);

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

  const selectedForReview = useMemo(() => {
    const selected = new Map<number, ImageQualityResult>();
    // Put the active view first so the dialog follows its order and score
    // scope. Other session selections follow without overwriting it.
    activeSequence?.images.forEach(image => {
      if (selectedImages.has(image.image_id)) selected.set(image.image_id, image);
    });
    sequences.forEach(sequence => {
      sequence.images.forEach(image => {
        if (selectedImages.has(image.image_id) && !selected.has(image.image_id)) {
          selected.set(image.image_id, image);
        }
      });
    });
    return Array.from(selected.values());
  }, [activeSequence, selectedImages, sequences]);

  // Batch rejection is deliberately two-step: show the exact per-image
  // evidence/reason before changing scheduler grades. Each image's own reason
  // is written — the scheduler keeps rejectreason per image, so a mixed batch
  // must not collapse to one shared string.
  const confirmRejectSelected = useCallback(async () => {
    if (!grading.canWrite || selectedForReview.length === 0) return;
    const byReason = new Map<string, number[]>();
    for (const img of selectedForReview) {
      const reason = img.regrade_reason ?? 'Quality analysis';
      const ids = byReason.get(reason);
      if (ids) ids.push(img.image_id);
      else byReason.set(reason, [img.image_id]);
    }
    for (const [reason, ids] of byReason) {
      await grading.gradeBatch(ids, 'rejected', reason);
    }
    setSelectedImages(new Set());
    selectionAnchorIdRef.current = null;
    selectionBaseIdsRef.current = new Set();
    setShowRejectReview(false);
  }, [grading, selectedForReview, setSelectedImages]);

  // Toggle individual image selection
  const toggleImage = useCallback((imageId: number) => {
    activeImageIdRef.current = imageId;
    selectionAnchorIdRef.current = imageId;
    setCurrentImageId(imageId);
    setSelectedImages(prev => {
      const next = new Set(prev);
      if (next.has(imageId)) {
        next.delete(imageId);
      } else {
        next.add(imageId);
      }
      const base = new Set(next);
      base.delete(imageId);
      selectionBaseIdsRef.current = base;
      return next;
    });
  }, [setCurrentImageId, setSelectedImages]);

  const selectImage = useCallback((imageId: number, event: React.MouseEvent) => {
    const storedAnchorId = selectionAnchorIdRef.current;
    const anchorId = storedAnchorId !== null && activeSequence?.images.some(
      image => image.image_id === storedAnchorId,
    ) ? storedAnchorId : activeImageIdRef.current;
    if (event.shiftKey && activeSequence && anchorId !== null) {
      const anchorIndex = activeSequence.images.findIndex(image => image.image_id === anchorId);
      const imageIndex = activeSequence.images.findIndex(image => image.image_id === imageId);
      if (anchorIndex >= 0 && imageIndex >= 0) {
        const start = Math.min(anchorIndex, imageIndex);
        const end = Math.max(anchorIndex, imageIndex);
        setSelectedImages(() => {
          const next = new Set(selectionBaseIdsRef.current);
          activeSequence.images.slice(start, end + 1).forEach(image => {
            next.add(image.image_id);
          });
          return next;
        });
        activeImageIdRef.current = imageId;
        setCurrentImageId(imageId);
        return;
      }
    }
    toggleImage(imageId);
  }, [activeSequence, setCurrentImageId, setSelectedImages, toggleImage]);

  const moveImageCursor = useCallback((direction: GridNavigationDirection) => {
    const currentImageId = activeImageIdRef.current;
    if (!activeSequence || currentImageId === null) return;
    const currentIndex = activeSequence.images.findIndex(
      image => image.image_id === currentImageId,
    );
    if (currentIndex < 0) return;
    const nextIndex = findGridNavigationIndex(
      activeSequence.images,
      currentIndex,
      direction,
      image => sequenceStripRef.current
        ?.querySelector<HTMLElement>(`[data-card-image-id="${image.image_id}"]`)
        ?.getBoundingClientRect() ?? null,
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

  useHotkeys('left', () => moveImageCursor('prev'), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('right', () => moveImageCursor('next'), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('up', () => moveImageCursor('up'), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('down', () => moveImageCursor('down'), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('space', () => {
    const currentImageId = activeImageIdRef.current;
    if (currentImageId !== null) toggleImage(currentImageId);
  }, sequenceHotkeyOptions, [toggleImage]);

  const openImage = useCallback((imageId: number) => {
    const scroller = document.querySelector<HTMLElement>('.app-main');
    const anchor = document.querySelector<HTMLElement>(
      `.sequence-image-card[data-card-image-id="${imageId}"]`,
    );
    const scrollTop = scroller?.scrollTop ?? 0;
    const offsetTop = scroller && anchor
      ? anchor.getBoundingClientRect().top - scroller.getBoundingClientRect().top
      : 0;
    navigate(imageDetailPath(imageId, searchParams, 'sequence'), {
      state: imageDetailNavigationState({ scrollTop, imageId, offsetTop }),
    });
  }, [navigate, searchParams]);

  const selectSequence = useCallback((choice: SequenceChoice) => {
    const sequence = choice.sequence;
    const firstImageId = sequence.images[0]?.image_id;
    if (firstImageId !== undefined) {
      selectionAnchorIdRef.current = firstImageId;
      const sequenceImageIds = new Set(sequence.images.map(image => image.image_id));
      selectionBaseIdsRef.current = new Set(
        Array.from(selectedImages).filter(imageId => !sequenceImageIds.has(imageId)),
      );
      const params = new URLSearchParams(searchParams);
      params.set('current', String(firstImageId));
      params.delete('groupIndex');
      params.delete('imageIndex');
      if (choice.scoreScope === 'target_filter') {
        params.set('scoreScope', `target-filter:${sequence.filter_name}`);
      } else {
        params.delete('scoreScope');
      }
      navigate(`/sequence?${params.toString()}`, { replace: true });
    }
  }, [navigate, searchParams, selectedImages]);

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
                    params.delete('scoreScope');
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
            <QualityScanButton
              scan={spatialScan}
              targetId={targetId}
              canWrite={grading.canWrite}
              className="header-button"
            />
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
          <ThumbnailSizeControl
            id="sequence-thumbnail-size"
            value={imageSize}
            onChange={setImageSize}
          />
          <div className="threshold-control">
            <label htmlFor="sequence-threshold">Score threshold:</label>
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
          <ScoringPenaltyControl />
          <SecondaryScoreToggle />
          <div className="selection-preset-control">
            <label htmlFor="sequence-select-preset">Select:</label>
            <select
              id="sequence-select-preset"
              value=""
              onChange={(event) => applySelectionPreset(event.target.value)}
            >
              <option value="" disabled>Choose images…</option>
              <option value="threshold">Below score threshold</option>
              <option value="clouded">Clouded</option>
              <option value="off-target">Off target</option>
              <option value="unsolved">Unsolved</option>
              <option value="recommended">Recommended</option>
            </select>
          </div>
          {selectedForReview.length > 0 && (
            <div className="sequence-selection-slot">
              <div className="selection-action-bar sequence-selection-bar" aria-label="Selected image actions">
                <span className="selection-count">{selectedForReview.length} selected</span>
                <button
                  type="button"
                  className="action-button reject"
                  disabled={!grading.canWrite}
                  onClick={() => setShowRejectReview(true)}
                >
                  Review rejection
                </button>
                <button
                  type="button"
                  className="header-button"
                  onClick={() => {
                    setSelectedImages(new Set());
                    selectionAnchorIdRef.current = null;
                    selectionBaseIdsRef.current = new Set();
                  }}
                >
                  Clear
                </button>
              </div>
            </div>
          )}
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
          {sequenceChoices.length > 1 && (
            <div className="sequence-tabs">
              {sequenceChoices.map(choice => {
                const seq = choice.sequence;
                const selectedCount = seq.images.filter(image =>
                  selectedImages.has(image.image_id)
                ).length;
                return (
                  <button
                    key={choice.key}
                    className={`sequence-tab ${choice.key === activeChoice?.key ? 'active' : ''}`}
                    onClick={() => selectSequence(choice)}
                  >
                    <span>{choice.label}</span>
                    {selectedCount > 0 && (
                      <span className="sequence-tab-selection-count">{selectedCount}</span>
                    )}
                  </button>
                );
              })}
            </div>
          )}

          {activeSequence && (
            <>
              {/* Summary bar */}
              <div className="sequence-summary-bar">
                <div className="summary-stats">
                  <span className="summary-item excellent">{activeSequence.summary.excellent_count} at 90–100</span>
                  <span className="summary-item good">{activeSequence.summary.good_count} at 70–89</span>
                  <span className="summary-item fair">{activeSequence.summary.fair_count} at 50–69</span>
                  <span className="summary-item poor">{activeSequence.summary.poor_count} at 30–49</span>
                  <span className="summary-item bad">{activeSequence.summary.bad_count} below 30</span>
                </div>
                <div className="sequence-score-context">
                  {activeScoreScope === 'target_filter'
                    ? `Stack comparison · matching capture settings across all sessions${
                      unavailableImageCount > 0
                        ? ` · ${unavailableImageCount} not comparable across sessions`
                        : ''
                    }`
                    : 'Session comparison · one capture run'}
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
                key={`${activeChoice?.key}-timeline`}
                images={activeSequence.images}
                scoreScope={activeScoreScope}
                threshold={threshold}
                currentImageId={activeImageId}
                selectedImages={selectedImages}
                onSelect={selectImage}
              />

              <PointingScatter images={activeSequence.images} />

              {/* Image strip */}
              <SequenceStrip
                key={`${activeChoice?.key}-strip`}
                dbId={dbId!}
                images={activeSequence.images}
                scoreScope={activeScoreScope}
                basisScoresByImage={basisScoresByImage}
                imageMap={imageMap}
                projectId={projectId!}
                targetId={activeSequence.target_id}
                targetName={activeSequence.target_name}
                filterName={activeSequence.filter_name}
                currentImageId={activeImageId}
                selectedImages={selectedImages}
                threshold={threshold}
                imageSize={imageSize}
                stripRef={sequenceStripRef}
                onSelect={selectImage}
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
        title={`Review ${selectedForReview.length} selected frame${selectedForReview.length === 1 ? '' : 's'}`}
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
              disabled={grading.isLoading || !grading.canWrite}
            >
              Reject selected ({selectedForReview.length})
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
  scoreScope,
  threshold,
  currentImageId,
  selectedImages,
  onSelect,
}: {
  images: ImageQualityResult[];
  scoreScope: QualityScoreScope;
  threshold: number;
  currentImageId: number | null;
  selectedImages: Set<number>;
  onSelect: (id: number, event: React.MouseEvent) => void;
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
          aria-label={scoreScope === 'target_filter'
            ? 'Target and filter stack comparison scores'
            : 'Capture sequence comparison scores'}
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
              data-image-id={img.image_id}
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
              onClick={(event) => onSelect(img.image_id, event)}
            >
              <title>{qualityScoreDescription(img, scoreScope)}{img.category ? ` ${formatCategory(img.category)}.` : ''}</title>
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
  scoreScope,
  basisScoresByImage,
  imageMap,
  projectId,
  targetId,
  targetName,
  filterName,
  currentImageId,
  selectedImages,
  threshold,
  imageSize,
  stripRef,
  onSelect,
  onOpen,
}: {
  dbId: string;
  images: ImageQualityResult[];
  scoreScope: QualityScoreScope;
  basisScoresByImage: ReadonlyMap<number, BasisScores>;
  imageMap: ReadonlyMap<number, Image>;
  projectId: number;
  targetId: number;
  targetName: string;
  filterName: string;
  currentImageId: number | null;
  selectedImages: Set<number>;
  threshold: number;
  imageSize: number;
  stripRef: RefObject<HTMLDivElement | null>;
  onSelect: (id: number, event: React.MouseEvent) => void;
  onOpen: (id: number) => void;
}) {
  useEffect(() => {
    if (currentImageId === null) return;
    requestAnimationFrame(() => {
      const currentCard = stripRef.current?.querySelector<HTMLElement>(
        '.sequence-image-card.current-selection',
      );
      const scroller = stripRef.current?.closest<HTMLElement>('.app-main');
      if (!currentCard || !scroller) return;
      const cardRect = currentCard.getBoundingClientRect();
      const scrollerRect = scroller.getBoundingClientRect();
      const isClipped =
        cardRect.top < scrollerRect.top || cardRect.bottom > scrollerRect.bottom;
      const isOutsideViewport =
        cardRect.bottom <= scrollerRect.top || cardRect.top >= scrollerRect.bottom;
      const fitsViewport = cardRect.height <= scrollerRect.height;
      if (isOutsideViewport || (fitsViewport && isClipped)) {
        currentCard.scrollIntoView({ block: 'nearest', inline: 'nearest' });
      }
    });
  }, [currentImageId, imageSize, stripRef]);

  return (
    <div
      ref={stripRef}
      className="filter-images sequence-strip"
      style={{
        gridTemplateColumns: thumbnailGridColumns(imageSize),
      }}
    >
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
            qualityScoreScope={scoreScope}
            basisScores={basisScoresByImage.get(quality.image_id)}
            isSelected={selectedImages.has(quality.image_id)}
            onClick={(event) => onSelect(quality.image_id, event)}
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
