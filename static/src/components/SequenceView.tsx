import {
  memo,
  useState,
  useMemo,
  useCallback,
  useEffect,
  useId,
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
import { useScoringPreferences } from '../hooks/useScoringPreferences';
import { useGrading } from '../hooks/useGrading';
import {
  useDbProjectTarget,
  useGridState,
  useImageOrder,
  useLatestSearchParams,
  useSequenceSelectionRule,
  useUrlParams,
} from '../hooks/useUrlState';
import { cardIsOnScreen, useReorderAnchor, viewIsAtRest } from '../hooks/useReorderAnchor';
import { useNewFrameIds } from '../hooks/useNewFrames';
import { inImageOrder, type ImageOrder } from '../utils/imageOrder';
import { useProjectMosaic } from '../hooks/useProjectMosaic';
import { useNightBoundary } from '../hooks/useNightBoundary';
import { timeMarks, timeSlotAttributes } from '../utils/timeBreaks';
import TimeBreakMark from './TimeBreakMark';
import TimeMarksChoice from './TimeMarksChoice';
import NightSplitChoice from './NightSplitChoice';
import { useDisplayPreferences } from '../hooks/useDisplayPreferences';
import SequenceCutoffControl from './SequenceCutoffControl';
import {
  cutoffMetricOf,
  formatCutoff,
  metricValue,
  pastCutoff,
  resolveCutoff,
  storedCutoff,
  type CutoffMetric,
  type CutoffScale,
} from '../utils/sequenceCutoff';
import MosaicSequence from './MosaicSequence';
import UndoRedoToolbar from './UndoRedoToolbar';
import ImageCard from './ImageCard';
import Dialog from './Dialog';
import ViewToolbar from './toolbar/ViewToolbar';
import PhoneUndoItems from './toolbar/PhoneUndoItems';
import RescanImageFolders from './toolbar/RescanImageFolders';
import SelectionBar from './toolbar/SelectionBar';
import PillSelect from './toolbar/PillSelect';
import ImageOrderToggle from './toolbar/ImageOrderToggle';
import ThumbnailSizeControl from './ThumbnailSizeControl';
import { QualityScanButton } from './QualityScanControl';
import { CalendarRange, ChevronRight, ScanSearch, X } from 'lucide-react';
import { SequenceIssueList, SequenceIssuePills } from './SequenceIssues';
import { frameTime } from '../utils/frameTime';
import ToolbarDock from './ToolbarDock';
import { sequenceIssues, type SequenceIssue } from '../utils/sequenceIssues';
import { GradingStatus } from '../api/types';
import type { CalibrationLightMark, Image, ScoredSequence, ImageQualityResult } from '../api/types';
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
import { useCalibrationMarks } from '../hooks/useCalibrationMarks';
import { useNavigateNow } from '../hooks/useNavigateNow';
import {
  type BasisScores,
  qualityScoreDescription,
  type QualityScoreScope,
} from '../utils/qualityScore';
import {
  followRule,
  handChanges,
  parseSelectionRule,
  ruleSelection,
  SELECTION_RULES,
  selectionRuleOf,
  type SelectionRuleId,
} from '../utils/sequenceSelectionRules';
import { keepAcrossNightSplit, nightSplitNote, useNightSplit } from '../hooks/useNightSplit';
import { pressesFocusedControl } from '../utils/focusedControl';

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

/** One rule's answer, as a key that changes only when the answer does. */
function rulePicksKey(rule: SelectionRuleId, picks: ReadonlySet<number>): string {
  return `${rule}:${[...picks].sort((a, b) => a - b).join(',')}`;
}

function qualityColor(score: number): string {
  if (score >= 0.7) return 'var(--color-score-good)';
  if (score >= 0.5) return 'var(--color-score-fair)';
  return 'var(--color-score-poor)';
}

/** The grade mark under a chart bar: the grid's accepted and rejected
 * colours, and a faint mark for a frame not graded yet. */
function gradeMark(image: Image): { grade: string; label: string; fill: string; opacity: number } {
  switch (image.grading_status) {
    case GradingStatus.Accepted:
      return { grade: 'accepted', label: 'Accepted', fill: 'var(--color-success)', opacity: 1 };
    case GradingStatus.Rejected: {
      const reason = image.reject_reason?.trim().replace(/\.$/, '');
      return {
        grade: 'rejected',
        label: reason ? `Rejected: ${reason}` : 'Rejected',
        fill: 'var(--color-error)',
        opacity: 1,
      };
    }
    default:
      return { grade: 'pending', label: 'Pending', fill: 'var(--color-text-muted)', opacity: 0.35 };
  }
}

export default function SequenceView() {
  const { dbId, projectId, targetId, mosaic: mosaicScope, setTargetId } = useDbProjectTarget();
  const { data: mosaic } = useProjectMosaic(mosaicScope ? dbId : null, mosaicScope ? projectId : null);
  const {
    imageSize,
    currentImageId: urlCurrentImageId,
    selectedImages,
    setImageSize,
    setCurrentImageId,
    setSelectedImages,
  } = useGridState('sequence');
  const location = useLocation();
  const [searchParams] = useSearchParams();
  // Writes start from the URL as it stands, not as this render shows it:
  // another write may have landed in the same moment.
  const { updateParams } = useUrlParams();
  const latestParams = useLatestSearchParams();
  const navigate = useNavigate();
  const navigateNow = useNavigateNow();
  const grading = useGrading(dbId!);
  // Calibration marks for the chips and the Poor calibration choice.
  const calibration = useCalibrationMarks(dbId, projectId, targetId != null);
  const filterName = searchParams.get('filterName') || undefined;
  const {
    data: analysisData,
    isLoading: isAnalyzing,
    error: analysisError,
  } = useSequenceAnalysis(dbId, targetId, filterName);
  const requestedScoreScope = searchParams.get('scoreScope');
  const { order, setOrder } = useImageOrder();
  const {
    metric: cutoffMetric,
    cutoff: urlCutoff,
    rule: selectionRule,
    setCutoff,
    setMetric: setCutoffMetric,
    setRule: setSelectionRule,
  } = useSequenceSelectionRule();
  const [showRejectReview, setShowRejectReview] = useState(false);
  const selectionAnchorIdRef = useRef<number | null>(null);
  const selectionBaseIdsRef = useRef<Set<number>>(new Set());
  // Whether the person has moved the cursor in the strip on screen, by keys
  // or a click. A new tab or scope opens at rest again.
  const touchedRef = useRef(false);
  // What the selection rule picked when it last ran: the frames that differ
  // from it were clicked by hand since. Null until the rule first runs.
  const ruleBaseRef = useRef<{ key: string; picks: Set<number> } | null>(null);
  const spatialScan = useSpatialScan(dbId, targetId ?? undefined, filterName);
  const { boundarySeconds: nightBoundarySeconds } = useNightBoundary(dbId);

  // Fetch targets for the project to allow selection
  const { data: targets = [] } = useQuery({
    queryKey: ['db', dbId, 'targets', projectId],
    queryFn: () => apiClient.getTargets(dbId!, projectId!),
    enabled: !!dbId && !!projectId,
  });

  // Fetch images for the target (for preview URLs)
  // Nights as the view splits them, the server's setting or the link's.
  const nightSplit = useNightSplit();
  const { data: images = [], isFetched: imagesFetched } = useQuery({
    queryKey: ['db', dbId, 'all-images', projectId, targetId, nightSplit.queryKey],
    queryFn: () =>
      apiClient.getImages(dbId!, {
        project_id: projectId || undefined,
        target_id: targetId || undefined,
        limit: 10000,
        nights: nightSplit.override ?? undefined,
      }),
    enabled: !!dbId && projectId !== undefined,
    // A new night split keeps the old frames on screen until its own
    // arrive, so the grid neither empties nor loses its place.
    placeholderData: keepAcrossNightSplit(['db', dbId, 'all-images', projectId, targetId]),
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
    // A mosaic's scope keeps every panel; it never narrows to one.
    if (!projectId || targetId || mosaicScope) return;
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
    mosaicScope,
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
      touchedRef.current = false;
      const inferredSelection = inferredSelectionRef.current;
      inferredSelectionRef.current = null;
      if (inferredSelection === null) entrySelectionRef.current = null;
      selectionAnchorIdRef.current = null;
      selectionBaseIdsRef.current = new Set();
      // A rule stays on in the new scope and selects there afresh.
      ruleBaseRef.current = null;
      setSelectedImages(inferredSelection ?? new Set());
    }
  }, [selectionScope, setSelectedImages]);

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
    inImageOrder(sequences, order).forEach(sequence => {
      const sessions = sessionsByFilter.get(sequence.filter_name);
      if (sessions) sessions.push(sequence);
      else sessionsByFilter.set(sequence.filter_name, [sequence]);
    });
    const sessionEvidence = new Map(
      sequences.flatMap(sequence => sequence.images).map(image => [image.image_id, image]),
    );

    // The server's sessions come in capture order, so the tabs read left
    // to right in it; newest first turns them around, the filter shot last
    // leading. Each filter's all-session view stays first.
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
  }, [order, rollups, sequences]);

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
  // The strip's frames in the order chosen; the score chart keeps capture
  // order, its time axis left to right.
  const orderedImages = useMemo(
    () => activeSequence ? inImageOrder(activeSequence.images, order) : [],
    [activeSequence, order],
  );
  // The suspect pills, each opening a list of its frames. A list belongs
  // to the view it was opened in.
  const issues = useMemo(() => sequenceIssues(orderedImages), [orderedImages]);
  const [openIssueState, setOpenIssueState] = useState<{ view: string; key: SequenceIssue['key'] } | null>(null);
  const openIssue = issues.find(issue =>
    issue.key === openIssueState?.key && openIssueState.view === activeChoice?.key,
  ) ?? null;
  const toggleIssue = useCallback((key: SequenceIssue['key']) => {
    setOpenIssueState(current => current?.key === key && current.view === activeChoice?.key
      ? null
      : { view: activeChoice?.key ?? '', key });
  }, [activeChoice?.key]);
  const activeScoreScope: QualityScoreScope = activeChoice?.scoreScope ?? 'capture_sequence';
  // The cutoff in the measure chosen: each frame's reading, the slider's
  // travel over the readings on screen, and the frames past the cutoff,
  // which the chart, the strip and the Below the cutoff rule share.
  const metricValues = useMemo(() => new Map(
    (activeSequence?.images ?? []).map(image => [
      image.image_id,
      metricValue(cutoffMetric, image, imageMap.get(image.image_id)),
    ]),
  ), [activeSequence, cutoffMetric, imageMap]);
  const observedValues = useMemo(
    () => [...metricValues.values()].filter((value): value is number => value !== null),
    [metricValues],
  );
  // A Stars or HFR cutoff is kept relative to the frames on screen, so it
  // means the same on another session tab or filter; its default moves
  // with the view's HFR-rise and star-loss penalties.
  const { hfrRise, starLoss } = useScoringPreferences();
  const cutoffPenalties = useMemo(() => ({ hfrRise, starLoss }), [hfrRise, starLoss]);
  const { scale: cutoffRange, value: threshold } = useMemo(
    () => resolveCutoff(cutoffMetric, urlCutoff, observedValues, cutoffPenalties),
    [cutoffMetric, cutoffPenalties, observedValues, urlCutoff],
  );
  const chooseCutoff = useCallback((value: number) => {
    setCutoff(storedCutoff(cutoffMetric, value, observedValues, cutoffPenalties));
  }, [cutoffMetric, cutoffPenalties, observedValues, setCutoff]);
  const pastCutoffIds = useMemo(() => {
    const ids = new Set<number>();
    if (threshold === null) return ids;
    metricValues.forEach((value, id) => {
      if (pastCutoff(cutoffMetric, value, threshold)) ids.add(id);
    });
    return ids;
  }, [cutoffMetric, metricValues, threshold]);
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
    if (orderedImages.length === 0) return null;
    if (urlCurrentImageId
      && orderedImages.some(image => image.image_id === urlCurrentImageId)) {
      return urlCurrentImageId;
    }
    return orderedImages[0].image_id;
  }, [orderedImages, urlCurrentImageId]);
  const activeImageIdRef = useRef(activeImageId);
  activeImageIdRef.current = activeImageId;
  const sequenceStripRef = useRef<HTMLDivElement>(null);
  // Turning the order around. At rest, the page stays put and the cursor
  // moves to the new first card. Once the person has moved, the current
  // card keeps its place on screen.
  const cardOf = useCallback((imageId: number | null) =>
    sequenceStripRef.current?.querySelector<HTMLElement>(
      `.sequence-image-card[data-card-image-id="${imageId}"]`,
    ) ?? null, []);
  // An order that changes by itself, such as a signed-in user's saved
  // order arriving after the strip drew, is a flip at rest while the view
  // is still as it opened.
  const captureReorder = useReorderAnchor(order, () => cardOf(activeImageIdRef.current), {
    firstId: orderedImages[0]?.image_id ?? null,
    decide: (previousFirstId) => {
      const nextFirst = orderedImages[0]?.image_id ?? null;
      if (touchedRef.current || previousFirstId === null || nextFirst === null
        || activeImageIdRef.current !== previousFirstId || nextFirst === previousFirstId) {
        return 'anchor';
      }
      activeImageIdRef.current = nextFirst;
      selectionAnchorIdRef.current = nextFirst;
      setCurrentImageId(nextFirst);
      return 'rest';
    },
  });
  const chooseOrder = useCallback((next: ImageOrder) => {
    if (next === order) return;
    const firstId = orderedImages[0]?.image_id ?? null;
    const atRest = viewIsAtRest({
      currentId: activeImageIdRef.current,
      firstId,
      touched: touchedRef.current,
      firstOnScreen: cardIsOnScreen(cardOf(firstId)),
    });
    const nextFirst = activeSequence ? inImageOrder(activeSequence.images, next)[0]?.image_id : undefined;
    if (!atRest || nextFirst === undefined) {
      captureReorder('anchor');
      setOrder(next);
      return;
    }
    captureReorder('rest');
    activeImageIdRef.current = nextFirst;
    selectionAnchorIdRef.current = nextFirst;
    setOrder(next, { current: nextFirst, groupIndex: null, imageIndex: null });
  }, [activeSequence, captureReorder, cardOf, order, orderedImages, setOrder]);
  // Frames captured since the last visit from the Library.
  const newImageIds = useNewFrameIds(dbId, images);
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

  // Write the cursor this render settled on, unless the address has moved
  // on since: a `current` written a moment ago, such as by an arrow key,
  // that this render does not show yet is newer than what it worked out.
  useEffect(() => {
    if (activeImageId === null || activeImageId === urlCurrentImageId) return;
    const scoreScope = activeScoreScope === 'target_filter' && !requestedScoreScope
      ? { scoreScope: `target-filter:${activeSequence?.filter_name ?? ''}` }
      : {};
    updateParams((latest) => {
      const current = latest.get('current');
      // Parsed as useGridState reads it, so an unreadable one still counts as the one shown.
      if (!Object.is(current ? parseInt(current, 10) : null, urlCurrentImageId)) return null;
      return { current: activeImageId, ...scoreScope, groupIndex: null, imageIndex: null };
    });
  }, [
    activeImageId,
    activeScoreScope,
    activeSequence?.filter_name,
    requestedScoreScope,
    updateParams,
    urlCurrentImageId,
  ]);

  // A selection set as a whole is the base of the next Shift-click range,
  // anchored on the image under the cursor.
  const anchorSelection = useCallback((ids: ReadonlySet<number>) => {
    const anchorId = activeImageIdRef.current;
    const base = new Set(ids);
    if (anchorId !== null) base.delete(anchorId);
    selectionAnchorIdRef.current = anchorId;
    selectionBaseIdsRef.current = base;
  }, []);

  // Get unique filter names from available targets
  const availableFilters = useMemo(() => {
    const filters = new Set<string>();
    images.forEach(img => {
      if (img.filter_name) filters.add(img.filter_name);
    });
    return Array.from(filters).sort();
  }, [images]);

  // Selection rules. A rule chosen under Select frames stays on: it lives
  // in the URL as selectRule and selects again whenever its answer
  // changes, as the cutoff moves, the scores or calibration marks reload,
  // or another session tab opens. It always answers for the view on screen.
  //
  // The rule and the person share the selection (see followRule): a click
  // by hand neither turns the rule off nor gets undone when the rule
  // selects again. Choosing "By hand" or Stop rule turns the rule off and
  // keeps the frames; Clear turns it off and empties the selection.
  // Rejecting the selection empties it and leaves the rule on: a rule never
  // picks a rejected frame, so it waits for the next frames to match.
  // Marks from before a reject, an import or a library change may no
  // longer hold, and a reject from Poor calibration writes the catalog: that
  // rule selects nothing until the pass that replaces them.
  const calibrationStale = projectId != null && calibration.staleProjects.has(projectId);
  const rejectedIds = useMemo(
    () => new Set(images.filter(image => image.grading_status === GradingStatus.Rejected).map(image => image.id)),
    [images],
  );
  // What a rule matches, grades aside, is what it follows (ruleBaseRef):
  // a reject, or an undo of one, changes no match, so neither makes the rule
  // select again. Rejected frames come off only what it selects and counts.
  const ruleMatchInputs = useMemo(
    () => ({ pastCutoff: pastCutoffIds, calibration: calibration.marks, calibrationStale }),
    [calibration.marks, calibrationStale, pastCutoffIds],
  );
  const ruleInputs = useMemo(
    () => ({ ...ruleMatchInputs, rejected: rejectedIds }),
    [ruleMatchInputs, rejectedIds],
  );
  const notRejected = useCallback(
    (ids: ReadonlySet<number>) => new Set([...ids].filter(id => !rejectedIds.has(id))),
    [rejectedIds],
  );
  const ruleCounts = useMemo(() => new Map(SELECTION_RULES.map(rule => [
    rule.id,
    activeSequence ? ruleSelection(rule.id, activeSequence.images, ruleInputs).size : 0,
  ])), [activeSequence, ruleInputs]);
  // Not before the grades load, so a rule never picks a rejected frame.
  const rulePicks = useMemo(
    () => selectionRule && activeSequence && !isAnalyzing && imagesFetched
      ? ruleSelection(selectionRule, activeSequence.images, ruleMatchInputs)
      : null,
    [activeSequence, imagesFetched, isAnalyzing, ruleMatchInputs, selectionRule],
  );
  const currentRulePicksKey = selectionRule && rulePicks ? rulePicksKey(selectionRule, rulePicks) : null;

  const applyRuleSelection = useCallback((update: (previous: Set<number>) => Set<number>) => {
    setSelectedImages(previous => {
      const next = update(previous);
      anchorSelection(next);
      return next;
    });
  }, [anchorSelection, setSelectedImages]);

  useEffect(() => {
    if (!selectionRule) {
      ruleBaseRef.current = null;
      return;
    }
    if (!rulePicks || currentRulePicksKey === null) return;
    const base = ruleBaseRef.current;
    if (base?.key === currentRulePicksKey) return;
    ruleBaseRef.current = { key: currentRulePicksKey, picks: rulePicks };
    applyRuleSelection(previous => {
      if (base) return notRejected(followRule(previous, base.picks, rulePicks));
      // Opened with a rule in the URL. With nothing selected the rule
      // selects afresh; a selection the URL carries stays as it was left,
      // so a reload or a trip to Detail keeps the clicks made by hand.
      return previous.size === 0 ? notRejected(rulePicks) : previous;
    });
  }, [applyRuleSelection, currentRulePicksKey, notRejected, rulePicks, selectionRule]);

  const chooseRule = useCallback((rule: SelectionRuleId | null) => {
    if (rule === null) {
      ruleBaseRef.current = null;
      setSelectionRule(null);
      return;
    }
    if (!activeSequence || !imagesFetched) {
      // Chosen while the scores or grades load: the rule selects once they arrive.
      ruleBaseRef.current = null;
      selectionAnchorIdRef.current = null;
      selectionBaseIdsRef.current = new Set();
      setSelectionRule(rule, new Set());
      return;
    }
    // A rule chosen replaces the selection with what it picks.
    const matches = ruleSelection(rule, activeSequence.images, ruleMatchInputs);
    ruleBaseRef.current = { key: rulePicksKey(rule, matches), picks: matches };
    const picks = notRejected(matches);
    anchorSelection(picks);
    setSelectionRule(rule, picks);
  }, [activeSequence, anchorSelection, imagesFetched, notRejected, ruleMatchInputs, setSelectionRule]);

  const clearSelection = useCallback(() => {
    ruleBaseRef.current = null;
    selectionAnchorIdRef.current = null;
    selectionBaseIdsRef.current = new Set();
    setSelectionRule(null, new Set());
  }, [setSelectionRule]);

  const selectedForReview = useMemo(() => {
    const selected = new Map<number, ImageQualityResult>();
    // Put the active view first so the dialog follows its order and score
    // scope. Other session selections follow without overwriting it.
    orderedImages.forEach(image => {
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
  }, [orderedImages, selectedImages, sequences]);

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
    // The rejected frames leave the selection; the rule stays on for the
    // frames that match it next, and passes these by.
    selectionAnchorIdRef.current = null;
    selectionBaseIdsRef.current = new Set();
    setSelectedImages(new Set());
    setShowRejectReview(false);
  }, [grading, selectedForReview, setSelectedImages]);

  // Toggle individual image selection
  const toggleImage = useCallback((imageId: number) => {
    touchedRef.current = true;
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

  // Shift-click selects the frames between the anchor and the click as the
  // strip shows them.
  const selectImage = useCallback((imageId: number, event: React.MouseEvent) => {
    touchedRef.current = true;
    const storedAnchorId = selectionAnchorIdRef.current;
    const anchorId = storedAnchorId !== null && orderedImages.some(
      image => image.image_id === storedAnchorId,
    ) ? storedAnchorId : activeImageIdRef.current;
    if (event.shiftKey && anchorId !== null) {
      const anchorIndex = orderedImages.findIndex(image => image.image_id === anchorId);
      const imageIndex = orderedImages.findIndex(image => image.image_id === imageId);
      if (anchorIndex >= 0 && imageIndex >= 0) {
        const start = Math.min(anchorIndex, imageIndex);
        const end = Math.max(anchorIndex, imageIndex);
        setSelectedImages(() => {
          const next = new Set(selectionBaseIdsRef.current);
          orderedImages.slice(start, end + 1).forEach(image => {
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
  }, [orderedImages, setCurrentImageId, setSelectedImages, toggleImage]);

  const moveImageCursor = useCallback((direction: GridNavigationDirection) => {
    const currentImageId = activeImageIdRef.current;
    if (currentImageId === null) return;
    const currentIndex = orderedImages.findIndex(
      image => image.image_id === currentImageId,
    );
    if (currentIndex < 0) return;
    const nextIndex = findGridNavigationIndex(
      orderedImages,
      currentIndex,
      direction,
      image => sequenceStripRef.current
        ?.querySelector<HTMLElement>(`[data-card-image-id="${image.image_id}"]`)
        ?.getBoundingClientRect() ?? null,
    );
    if (nextIndex === currentIndex) return;
    touchedRef.current = true;
    const nextImageId = orderedImages[nextIndex].image_id;
    activeImageIdRef.current = nextImageId;
    setCurrentImageId(nextImageId);
  }, [orderedImages, setCurrentImageId]);

  // From an issue's list or a pointing dot: the cursor moves to the frame,
  // which scrolls into view; the selection stays as it is.
  const goToImage = useCallback((imageId: number) => {
    touchedRef.current = true;
    activeImageIdRef.current = imageId;
    setCurrentImageId(imageId);
  }, [setCurrentImageId]);

  const sequenceHotkeyOptions = useMemo(() => ({
    enabled: !!activeSequence && !isAnalyzing && !showRejectReview,
    preventDefault: true,
  }), [activeSequence, isAnalyzing, showRejectReview]);
  // Space presses a focused button, such as a session tab or a suspect
  // pill, rather than toggling the frame under the cursor.
  const sequenceSpaceOptions = useMemo(
    () => ({ ...sequenceHotkeyOptions, ignoreEventWhen: pressesFocusedControl }),
    [sequenceHotkeyOptions],
  );

  useHotkeys('left', () => moveImageCursor('prev'), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('right', () => moveImageCursor('next'), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('up', () => moveImageCursor('up'), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('down', () => moveImageCursor('down'), sequenceHotkeyOptions, [moveImageCursor]);
  useHotkeys('space', () => {
    const currentImageId = activeImageIdRef.current;
    if (currentImageId !== null) toggleImage(currentImageId);
  }, sequenceSpaceOptions, [toggleImage]);

  const openImage = useCallback((imageId: number, calibrationFocus?: string) => {
    const scroller = document.querySelector<HTMLElement>('.app-main');
    const anchor = document.querySelector<HTMLElement>(
      `.sequence-image-card[data-card-image-id="${imageId}"]`,
    );
    const scrollTop = scroller?.scrollTop ?? 0;
    const offsetTop = scroller && anchor
      ? anchor.getBoundingClientRect().top - scroller.getBoundingClientRect().top
      : 0;
    navigateNow(imageDetailPath(imageId, latestParams(), 'sequence', calibrationFocus), {
      state: imageDetailNavigationState({ scrollTop, imageId, offsetTop }),
    });
  }, [latestParams, navigateNow]);

  const selectSequence = useCallback((choice: SequenceChoice) => {
    const sequence = choice.sequence;
    // A tab opens on its first frame as the strip shows it.
    const firstImageId = inImageOrder(sequence.images, order)[0]?.image_id;
    if (firstImageId !== undefined) {
      // The tab opens on its first frame, at rest.
      touchedRef.current = false;
      selectionAnchorIdRef.current = firstImageId;
      const sequenceImageIds = new Set(sequence.images.map(image => image.image_id));
      selectionBaseIdsRef.current = new Set(
        Array.from(selectedImages).filter(imageId => !sequenceImageIds.has(imageId)),
      );
      updateParams({
        current: firstImageId,
        groupIndex: null,
        imageIndex: null,
        scoreScope: choice.scoreScope === 'target_filter' ? `target-filter:${sequence.filter_name}` : null,
      });
    }
  }, [order, selectedImages, updateParams]);

  const chooseFilter = useCallback((filter: string | undefined) => {
    const params = latestParams();
    if (filter) {
      params.set('filterName', filter);
    } else {
      params.delete('filterName');
    }
    params.delete('scoreScope');
    navigate(`/sequence?${params.toString()}`);
  }, [latestParams, navigate]);

  const ruleHandChanges = rulePicks ? handChanges(selectedImages, notRejected(rulePicks)) : 0;

  if (!projectId) {
    return (
      <div className="empty-state">
        Select a project to analyze image sequences
      </div>
    );
  }

  if (!targetId && mosaic && dbId) {
    return <MosaicSequence dbId={dbId} mosaic={mosaic} images={images} />;
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
                    const params = latestParams();
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
      <h2 className="visually-hidden">Sequence Analysis</h2>
      <ViewToolbar
        label="Sequence tools"
        className="sequence-controls"
        sticky
        filters={availableFilters.length > 1 ? (
          <PillSelect
            label="Filter"
            id="sequence-filter"
            active={!!filterName}
            value={filterName || 'all'}
            onChange={(e) => chooseFilter(e.target.value === 'all' ? undefined : e.target.value)}
          >
            <option value="all">All</option>
            {availableFilters.map(f => (
              <option key={f} value={f}>{f}</option>
            ))}
          </PillSelect>
        ) : undefined}
        // One filter pill: choosing All clears it.
        activeFilters={filterName ? 1 : 0}
        controls={(
          <>
            <ThumbnailSizeControl
              id="sequence-thumbnail-size"
              value={imageSize}
              onChange={setImageSize}
              icons
            />
            <SequenceCutoffControl
              metric={cutoffMetric}
              scale={cutoffRange}
              value={threshold}
              onMetricChange={setCutoffMetric}
              onChange={chooseCutoff}
            />
            <ScoringPenaltyControl />
          </>
        )}
        primary={(
          <>
            <PillSelect
              label="Select"
              aria-label="Select frames"
              id="sequence-select-rule"
              pillClassName="sequence-select-rule"
              active={!!selectionRule}
              pillTitle={selectionRule
                ? `${selectionRuleOf(selectionRule, cutoffMetric).description}. The rule stays on: it selects again when the cutoff or the scores change.`
                : 'Choose a rule to select frames. It stays on, and selects again when the cutoff or the scores change.'}
              value={selectionRule ?? ''}
              onChange={(e) => chooseRule(parseSelectionRule(e.target.value))}
            >
              <option value="">By hand</option>
              {SELECTION_RULES.map(rule => (
                rule.id === 'poor-calibration' && calibrationStale
                  ? (
                    <option key={rule.id} value={rule.id} title={rule.description} disabled>
                      {rule.label} (updating)
                    </option>
                  )
                  : (
                    <option
                      key={rule.id}
                      value={rule.id}
                      title={selectionRuleOf(rule.id, cutoffMetric).description}
                    >
                      {selectionRuleOf(rule.id, cutoffMetric).label} ({ruleCounts.get(rule.id) ?? 0})
                    </option>
                  )
              ))}
            </PillSelect>
            {selectionRule && (
              <button
                type="button"
                className="tool-button tool-button-quiet sequence-stop-rule"
                aria-label="Stop rule"
                title="Turn the rule off; the frames it chose stay selected"
                onClick={() => chooseRule(null)}
              >
                <X size={14} aria-hidden="true" />
              </button>
            )}
          </>
        )}
        viewNote={[
          order === 'newest' ? 'newest first' : undefined,
          nightSplitNote(nightSplit.override, nightSplit.setting),
        ].filter(Boolean).join(', ') || undefined}
        view={(
          <>
            <ImageOrderToggle
              order={order}
              onChange={chooseOrder}
              title="The frames and the session tabs by capture time. The score chart keeps time left to right."
            />
            <TimeMarksChoice id="sequence-time-marks" />
            <NightSplitChoice id="sequence-night-split" />
            <div className="toolbar-field">
              <span className="toolbar-caption">Chips</span>
              <SecondaryScoreToggle />
            </div>
          </>
        )}
        tools={(
          <>
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
            <QualityScanButton
              scan={spatialScan}
              targetId={targetId}
              canWrite={grading.canWrite}
              className="tool-button icon-at-1360 quality-scan-button"
              icon={<ScanSearch size={14} aria-hidden="true" />}
            />
          </>
        )}
        // On a phone the cutoff slider folds away; its value stays in sight.
        menu={grading.canWrite ? (
          <>
            <PhoneUndoItems
              canUndo={grading.canUndo}
              canRedo={grading.canRedo}
              isProcessing={grading.isLoading}
              onUndo={grading.undo}
              onRedo={grading.redo}
            />
            <RescanImageFolders dbId={dbId ?? null} />
          </>
        ) : undefined}
        summary={threshold !== null && (
          <span className="toolbar-phone-only">
            {cutoffMetricOf(cutoffMetric).label} cutoff {formatCutoff(cutoffMetric, threshold)}
          </span>
        )}
        selection={(
          <SelectionBar
            className="sequence-selection-bar"
            count={selectedForReview.length}
            hint={selectionRule
              ? `No frames match “${selectionRuleOf(selectionRule, cutoffMetric).label}”`
              : 'Click frames, or choose a rule under Select frames'}
            note={selectionRule && (
              <>
                Rule: {selectionRuleOf(selectionRule, cutoffMetric).label}
                {ruleHandChanges > 0 && ` · ${ruleHandChanges} changed by hand`}
              </>
            )}
            onClear={clearSelection}
          >
            <button
              type="button"
              className="tool-button tool-button-reject"
              disabled={!grading.canWrite}
              title="Check each frame's evidence, then reject them"
              onClick={() => setShowRejectReview(true)}
            >
              <span className="tool-button-label">Review rejection</span>
            </button>
          </SelectionBar>
        )}
      />

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
              {sequenceChoices.map((choice, index) => {
                const seq = choice.sequence;
                // Each filter's tabs start a row of their own.
                const startsFilter = index === 0
                  || sequenceChoices[index - 1].sequence.filter_name !== seq.filter_name;
                const tabClass = [
                  'sequence-tab',
                  choice.scoreScope === 'target_filter' ? 'sequence-tab-all-sessions' : '',
                  startsFilter ? 'sequence-tab-filter-start' : '',
                ].filter(Boolean).join(' ');
                const selectedCount = seq.images.filter(image =>
                  selectedImages.has(image.image_id)
                ).length;
                return (
                  <button
                    key={choice.key}
                    className={tabClass}
                    aria-pressed={choice.key === activeChoice?.key}
                    onClick={() => selectSequence(choice)}
                  >
                    <span>
                      {choice.scoreScope === 'target_filter' && <CalendarRange size={14} aria-hidden="true" />}
                      {choice.label}
                    </span>
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
              {/* The summary, its suspect list and the chart stay under
                  the toolbar while the strip scrolls. */}
              <ToolbarDock>
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
                  <SequenceIssuePills issues={issues} open={openIssue?.key ?? null} onToggle={toggleIssue} />
                </div>
                {openIssue && (
                  <SequenceIssueList
                    issue={openIssue}
                    imageMap={imageMap}
                    currentImageId={activeImageId}
                    onGo={goToImage}
                    onClose={() => setOpenIssueState(null)}
                  />
                )}

                {/* Timeline chart */}
                <SequenceTimeline
                  key={`${activeChoice?.key}-timeline`}
                  images={activeSequence.images}
                  imageMap={imageMap}
                  scoreScope={activeScoreScope}
                  metric={cutoffMetric}
                  metricValues={metricValues}
                  scale={cutoffRange}
                  threshold={threshold}
                  pastCutoffIds={pastCutoffIds}
                  currentImageId={activeImageId}
                  selectedImages={selectedImages}
                  onSelect={selectImage}
                />
              </ToolbarDock>

              <PointingScatter
                images={activeSequence.images}
                imageMap={imageMap}
                currentImageId={activeImageId}
                onGo={goToImage}
              />

              {/* Image strip */}
              <SequenceStrip
                key={`${activeChoice?.key}-strip`}
                dbId={dbId!}
                images={orderedImages}
                newImageIds={newImageIds}
                scoreScope={activeScoreScope}
                basisScoresByImage={basisScoresByImage}
                calibrationByImage={calibration.marks}
                imageMap={imageMap}
                projectId={projectId!}
                targetId={activeSequence.target_id}
                targetName={activeSequence.target_name}
                filterName={activeSequence.filter_name}
                currentImageId={activeImageId}
                selectedImages={selectedImages}
                pastCutoffIds={pastCutoffIds}
                imageSize={imageSize}
                nightBoundarySeconds={nightBoundarySeconds}
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
              <strong>Image {image.image_id}</strong> · score {Math.round(image.quality_score * 100)}
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

/**
 * Whether the solved pointing chart starts folded. Like the stack panel's,
 * this is how someone reads the view rather than anything about a frame, so
 * it survives reloads. Storage can be unavailable (private browsing); the
 * chart then starts open.
 */
const POINTING_COLLAPSED_KEY = 'psf-guard.sequence-pointing-collapsed';

function readPointingCollapsed(): boolean {
  try {
    return window.localStorage.getItem(POINTING_COLLAPSED_KEY) === 'true';
  } catch {
    return false;
  }
}

const POINTING_FLAGS = new Set(['off_target', 'pointing_jump', 'pointing_drift', 'rotation_skew']);

/** An offset as a screen reader says it: "12 arcsec east". */
function offsetWords(value: number, positive: string, negative: string): string {
  return `${Math.abs(value).toFixed(0)} arcsec ${value < 0 ? negative : positive}`;
}

const PointingScatter = memo(function PointingScatter({
  images,
  imageMap,
  currentImageId,
  onGo,
}: {
  images: ImageQualityResult[];
  /** The catalog rows, for each frame's capture time. */
  imageMap: ReadonlyMap<number, Image>;
  currentImageId: number | null;
  /** Move the cursor to a frame, as a suspect list's frame does. */
  onGo: (imageId: number) => void;
}) {
  const [collapsed, setCollapsed] = useState(readPointingCollapsed);
  const toggleCollapsed = () => setCollapsed((current) => {
    const next = !current;
    try {
      window.localStorage.setItem(POINTING_COLLAPSED_KEY, String(next));
    } catch {
      // Keep the in-memory choice even when it cannot be persisted.
    }
    return next;
  });
  const titleId = useId();
  const bodyId = useId();
  const chartRef = useRef<SVGSVGElement>(null);

  // A dot with focus follows the cursor as the arrows move it, so the
  // next Enter goes on from there.
  useEffect(() => {
    const chart = chartRef.current;
    if (!chart || !chart.contains(document.activeElement)) return;
    chart.querySelector<SVGGElement>(`[data-pointing-image-id="${currentImageId}"]`)
      ?.focus({ preventScroll: true });
  }, [currentImageId]);

  const points = images.flatMap(image => {
    const east = image.pointing?.east_offset_arcsec;
    const north = image.pointing?.north_offset_arcsec;
    return east != null && north != null
      ? [{ image, east, north, flagged: (image.flags ?? []).some(flag => POINTING_FLAGS.has(flag)) }]
      : [];
  });
  if (points.length < 2) return null;
  const expectedTarget = points.some(point => point.image.pointing?.expected_target);
  const flaggedCount = points.filter(point => point.flagged).length;

  const extent = Math.max(
    30,
    ...points.flatMap(point => [Math.abs(point.east), Math.abs(point.north)])
  ) * 1.1;
  const size = 180;
  const center = size / 2;
  const project = (value: number) => center + (value / extent) * (center - 15);
  // The cursor's dot is drawn last, over its neighbours, and takes the
  // chart's one tab stop; without one, the first dot does.
  const current = points.find(point => point.image.image_id === currentImageId);
  const drawn = current ? [...points.filter(point => point !== current), current] : points;
  const tabStopId = current?.image.image_id ?? points[0].image.image_id;

  return (
    <section className="pointing-scatter" aria-labelledby={titleId}>
      <h3 id={titleId}>
        <button
          type="button"
          className="pointing-scatter-toggle"
          aria-expanded={!collapsed}
          aria-controls={collapsed ? undefined : bodyId}
          onClick={toggleCollapsed}
        >
          <ChevronRight size={14} aria-hidden="true" />
          Solved pointing
        </button>
        {collapsed && (
          <small>
            {points.length} solved frames
            {flaggedCount > 0 && ` · ${flaggedCount} off target, jumped or drifting`}
          </small>
        )}
      </h3>
      {!collapsed && (
        <div id={bodyId} className="pointing-scatter-body">
          <svg
            ref={chartRef}
            width={size}
            height={size}
            viewBox={`0 0 ${size} ${size}`}
            role="group"
            aria-label="Solved pointing offsets"
          >
            <rect x="0" y="0" width={size} height={size} fill="var(--color-bg-secondary)" rx="6" />
            <line x1={center} y1="10" x2={center} y2={size - 10} stroke="var(--color-text-muted)" opacity="0.45" />
            <line x1="10" y1={center} x2={size - 10} y2={center} stroke="var(--color-text-muted)" opacity="0.45" />
            {drawn.map(({ image, east, north, flagged }) => {
              const label = `${frameTime(imageMap.get(image.image_id))}: ${
                offsetWords(east, 'east', 'west')}, ${offsetWords(north, 'north', 'south')}`;
              const go = () => onGo(image.image_id);
              return (
                <g
                  key={image.image_id}
                  className="pointing-dot"
                  data-pointing-image-id={image.image_id}
                  role="button"
                  tabIndex={image.image_id === tabStopId ? 0 : -1}
                  aria-label={label}
                  aria-current={image === current?.image ? 'true' : undefined}
                  onClick={go}
                  onKeyDown={(event) => {
                    if (event.key !== 'Enter' && event.key !== ' ') return;
                    event.preventDefault();
                    go();
                  }}
                >
                  <title>{label}</title>
                  {/* A larger target than the dot, so a small one is easy to hit. */}
                  <circle className="pointing-dot-hit" cx={project(east)} cy={project(-north)} r={9} />
                  <circle
                    className="pointing-dot-mark"
                    cx={project(east)}
                    cy={project(-north)}
                    r={flagged ? 5 : 3}
                    fill={qualityColor(image.quality_score)}
                  />
                </g>
              );
            })}
            <text x={size - 12} y={center - 4} textAnchor="end" fontSize="9" fill="var(--color-text-muted)">E</text>
            <text x={center + 4} y="12" fontSize="9" fill="var(--color-text-muted)">N</text>
          </svg>
          <p className="pointing-scatter-legend">
            {expectedTarget ? 'Target' : 'First solved center'} is the crosshair. Range ±{extent.toFixed(0)}″.<br />
            Large points are off-target, jumps, or drift. Click a point to go to its frame.
          </p>
        </div>
      )}
    </section>
  );
});

// Timeline visualization component
const SequenceTimeline = memo(function SequenceTimeline({
  images,
  imageMap,
  scoreScope,
  metric,
  metricValues,
  scale,
  threshold,
  pastCutoffIds,
  currentImageId,
  selectedImages,
  onSelect,
}: {
  images: ImageQualityResult[];
  /** The catalog rows, for each frame's grade; they reload after grading. */
  imageMap: ReadonlyMap<number, Image>;
  scoreScope: QualityScoreScope;
  /** What the bars measure, and each frame's reading in it. */
  metric: CutoffMetric;
  metricValues: ReadonlyMap<number, number | null>;
  /** The slider's travel, which the Stars and HFR axis spans. */
  scale: CutoffScale | null;
  /** The cutoff in the measure's units, or null when no frame has a
   *  reading to set one from. */
  threshold: number | null;
  pastCutoffIds: ReadonlySet<number>;
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
  const padding = { top: 10, right: 10, bottom: 20, left: 36 };
  const innerWidth = chartWidth - padding.left - padding.right;
  const innerHeight = chartHeight - padding.top - padding.bottom;

  // The score runs 0 to 100. Stars and HFR span the slider's range, with
  // room below the lowest reading so its bar still shows: a session's HFR
  // moves a few tenths, which bars from zero would flatten. The axis
  // labels say where it starts.
  const span = scale ? scale.max - scale.min : 0;
  const axisMin = metric === 'score' || !scale ? 0 : Math.max(0, scale.min - span * 0.25);
  const axisMax = metric === 'score' ? 100 : scale ? scale.max + span * 0.05 : 1;
  const axisLabel = (value: number) => metric === 'hfr' ? value.toFixed(2) : String(Math.round(value));
  const yOf = (value: number) => padding.top
    + innerHeight * (1 - Math.min(1, Math.max(0, (value - axisMin) / (axisMax - axisMin))));
  const measureLabel = metric === 'score' ? 'scores' : metric === 'stars' ? 'detected stars' : 'HFR';

  const barStep = innerWidth / images.length;
  const barWidth = Math.max(0.75, Math.min(10, barStep * 0.8));
  // The grade marks sit in the empty strip under the bars. A mark is a dot
  // until the bars get thinner than it, then a pill as wide as its bar, so
  // it never reaches a neighbour's.
  const markHeight = 6;
  const markWidth = Math.min(markHeight, barWidth, barStep);
  const markY = padding.top + innerHeight + 4;

  return (
    <div className="sequence-timeline">
      <div ref={viewportRef} className="sequence-timeline-scroll">
        <svg
          width={chartWidth}
          height={chartHeight}
          viewBox={`0 0 ${chartWidth} ${chartHeight}`}
          role="img"
          aria-label={metric !== 'score'
            ? `Frame ${measureLabel} through the sequence`
            : scoreScope === 'target_filter'
              ? 'Target and filter stack comparison scores'
              : 'Capture sequence comparison scores'}
          data-metric={metric}
        >
        {/* Y-axis labels */}
        <text x={padding.left - 4} y={padding.top + 4} fontSize="9" fill="var(--color-text-muted)" textAnchor="end">{axisLabel(axisMax)}</text>
        <text x={padding.left - 4} y={padding.top + innerHeight / 2 + 3} fontSize="9" fill="var(--color-text-muted)" textAnchor="end">{axisLabel((axisMin + axisMax) / 2)}</text>
        <text x={padding.left - 4} y={padding.top + innerHeight + 3} fontSize="9" fill="var(--color-text-muted)" textAnchor="end">{axisLabel(axisMin)}</text>

        {/* Bars */}
        {images.map((img, i) => {
          const x = padding.left + i * barStep + (barStep - barWidth) / 2;
          const reading = metricValues.get(img.image_id) ?? null;
          const barHeight = reading === null ? 0 : padding.top + innerHeight - yOf(reading);
          const y = padding.top + innerHeight - barHeight;
          // The score keeps its good, fair and poor colours. Stars and HFR
          // show the frames past the cutoff in the poor colour.
          const barFill = metric === 'score'
            ? qualityColor(img.quality_score)
            : pastCutoffIds.has(img.image_id) ? 'var(--color-score-poor)' : 'var(--color-chart-line)';
          const isSelected = selectedImages.has(img.image_id);
          const isCurrent = currentImageId === img.image_id;
          const image = imageMap.get(img.image_id);
          const mark = image ? gradeMark(image) : null;
          const readingText = metric === 'score'
            ? ''
            : ` ${metric === 'stars' ? 'Stars' : 'HFR'} ${reading === null ? 'not recorded' : formatCutoff(metric, reading)}.`;
          const title = `${mark ? `${mark.label}. ` : ''}${qualityScoreDescription(img, scoreScope)}${readingText}${
            img.category ? ` ${formatCategory(img.category)}.` : ''
          }`;

          return (
            <g key={img.image_id}>
              <rect
                data-image-id={img.image_id}
                x={x}
                y={y}
                width={barWidth}
                height={Math.max(1, barHeight)}
                fill={barFill}
                opacity={isSelected || isCurrent ? 1 : 0.7}
                stroke={isCurrent
                  ? 'var(--state-on-border)'
                  : isSelected ? 'var(--state-picked)' : 'none'}
                strokeWidth={isCurrent ? 2 : isSelected ? 1 : 0}
                style={{ cursor: 'pointer' }}
                onClick={(event) => onSelect(img.image_id, event)}
              >
                <title>{title}</title>
              </rect>
              {mark && (
                <rect
                  className="sequence-grade-mark"
                  data-grade-image-id={img.image_id}
                  data-grade={mark.grade}
                  x={padding.left + i * barStep + (barStep - markWidth) / 2}
                  y={markY}
                  width={markWidth}
                  height={markHeight}
                  rx={markWidth / 2}
                  fill={mark.fill}
                  opacity={mark.opacity}
                  style={{ cursor: 'pointer' }}
                  onClick={(event) => onSelect(img.image_id, event)}
                >
                  <title>{title}</title>
                </rect>
              )}
            </g>
          );
        })}
        {/* The cutoff line, over the bars so it shows where they cross it */}
        {threshold !== null && (
          <line
            className="sequence-cutoff-line"
            x1={padding.left}
            y1={yOf(threshold)}
            x2={chartWidth - padding.right}
            y2={yOf(threshold)}
            stroke="var(--color-warning)"
            strokeWidth="1"
            strokeDasharray="4,4"
            opacity="0.8"
            pointerEvents="none"
          />
        )}
        </svg>
      </div>
    </div>
  );
});

const SequenceStrip = memo(function SequenceStrip({
  dbId,
  images,
  newImageIds,
  scoreScope,
  basisScoresByImage,
  calibrationByImage,
  imageMap,
  projectId,
  targetId,
  targetName,
  filterName,
  currentImageId,
  selectedImages,
  pastCutoffIds,
  imageSize,
  nightBoundarySeconds,
  stripRef,
  onSelect,
  onOpen,
}: {
  dbId: string;
  /** The frames in the order chosen. */
  images: ImageQualityResult[];
  /** Frames captured since the last visit from the Library. */
  newImageIds: ReadonlySet<number>;
  scoreScope: QualityScoreScope;
  basisScoresByImage: ReadonlyMap<number, BasisScores>;
  calibrationByImage: ReadonlyMap<number, CalibrationLightMark>;
  imageMap: ReadonlyMap<number, Image>;
  projectId: number;
  targetId: number;
  targetName: string;
  filterName: string;
  currentImageId: number | null;
  selectedImages: Set<number>;
  /** Frames past the cutoff in the measure chosen, marked on their cards. */
  pastCutoffIds: ReadonlySet<number>;
  imageSize: number;
  /** Where the catalog's nights split, for the night marks between cards. */
  nightBoundarySeconds: number;
  stripRef: RefObject<HTMLDivElement | null>;
  onSelect: (id: number, event: React.MouseEvent) => void;
  /** Open a light, on its calibration account with a finding chosen. */
  onOpen: (id: number, calibrationFocus?: string) => void;
}) {
  useEffect(() => {
    if (currentImageId === null) return;
    // Brings the cursor's card into sight; true when it had to scroll.
    const reveal = () => {
      const currentCard = stripRef.current?.querySelector<HTMLElement>(
        '.sequence-image-card.current-selection',
      );
      const scroller = stripRef.current?.closest<HTMLElement>('.app-main');
      if (!currentCard || !scroller) return false;
      const cardRect = currentCard.getBoundingClientRect();
      const scrollerRect = scroller.getBoundingClientRect();
      // The toolbar and the docked chart cover the top of the page: a card
      // under them is out of sight, and one brought back stops below them.
      const covers = stripRef.current?.parentElement?.querySelectorAll<HTMLElement>(
        ':scope > .view-toolbar.sticky, :scope > .toolbar-dock',
      ) ?? [];
      const visibleTop = Math.max(
        scrollerRect.top,
        ...Array.from(covers, cover => cover.getBoundingClientRect().bottom),
      );
      const isClipped =
        cardRect.top < visibleTop - 1 || cardRect.bottom > scrollerRect.bottom + 1;
      const isOutsideViewport =
        cardRect.bottom <= visibleTop || cardRect.top >= scrollerRect.bottom;
      const fitsViewport = cardRect.height <= scrollerRect.bottom - visibleTop;
      if (!isOutsideViewport && !(fitsViewport && isClipped)) return false;
      // As scrollIntoView's nearest, measured from below the covers.
      const gap = 8;
      scroller.scrollTop += cardRect.top < visibleTop || !fitsViewport
        ? cardRect.top - visibleTop - gap
        : cardRect.bottom - scrollerRect.bottom + gap;
      return true;
    };
    // Cards lay themselves out as they come into sight (content-visibility),
    // a frame or two after the scroll that brought them, and Safari has no
    // scroll anchoring to make up for it: they can move this card again.
    // After a scroll, look again until it has held still for a few frames.
    let frame = 0;
    let checks = 0;
    let still = 0;
    const check = () => {
      checks += 1;
      still = reveal() ? 0 : still + 1;
      if (still < 4 && checks < 20) frame = requestAnimationFrame(check);
    };
    frame = requestAnimationFrame(() => {
      if (reveal()) frame = requestAnimationFrame(check);
    });
    return () => cancelAnimationFrame(frame);
  }, [currentImageId, imageSize, stripRef]);

  const { timeMarks: marksChoice = 'nights-hours' } = useDisplayPreferences();
  const marks = useMemo(
    () => marksChoice === 'off' ? null : timeMarks(
      images.map((quality) => imageMap.get(quality.image_id) ?? { acquired_date: null }),
      nightBoundarySeconds,
      { hours: marksChoice === 'nights-hours' },
    ),
    [imageMap, images, marksChoice, nightBoundarySeconds],
  );

  return (
    <div
      ref={stripRef}
      className="filter-images sequence-strip"
      style={{
        gridTemplateColumns: thumbnailGridColumns(imageSize),
      }}
    >
      {images.map((quality, index) => {
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
        const belowThreshold = pastCutoffIds.has(quality.image_id);
        const slot = timeSlotAttributes(marks, index);
        return (
          <div key={quality.image_id} className={slot.className} data-time-break={slot['data-time-break']}>
            <TimeBreakMark marks={marks} index={index} />
            <ImageCard
              dbId={dbId}
              image={image}
              quality={quality}
              qualityScoreScope={scoreScope}
              basisScores={basisScoresByImage.get(quality.image_id)}
              calibration={calibrationByImage.get(quality.image_id)}
              onCalibrationClick={(mark) => onOpen(quality.image_id, mark.codes[0])}
              isSelected={selectedImages.has(quality.image_id)}
              isNew={newImageIds.has(quality.image_id)}
              onClick={(event) => onSelect(quality.image_id, event)}
              onDoubleClick={() => onOpen(quality.image_id)}
              lazyPreview
              selectionEffects={false}
              showTarget={false}
              className={`sequence-image-card${
                currentImageId === quality.image_id ? ' current-selection' : ''
              }${belowThreshold ? ' below-threshold' : ''}`}
            />
          </div>
        );
      })}
    </div>
  );
});
