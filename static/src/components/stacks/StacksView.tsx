import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useDbProjectTarget, useLatestSearchParams, useUrlParams } from '../../hooks/useUrlState';
import { useAccess } from '../../auth/access';
import ProjectExposureGrouping from '../ProjectExposureGrouping';
import { imageDetailPath } from '../../utils/imageDetailRoutes';
import StackPreviewPanel from '../StackPreviewPanel';
import ThumbnailSizeControl from '../ThumbnailSizeControl';
import WbppRunDialog from '../WbppRunDialog';
import { describeWbppRunForProject, useWbppRun } from '../../hooks/useWbppRun';
import { DEFAULT_WBPP_OPTIONS } from '../../api/types';
import WbppStacks from './WbppStacks';
import MosaicStacks from './MosaicStacks';
import { useProjectMosaic } from '../../hooks/useProjectMosaic';
import { keepAcrossNightSplit, useNightSplit } from '../../hooks/useNightSplit';
import './stacks.css';

/** What Images hands over when a person stacks a selection. */
export interface StackSelectionState {
  stackImageIds?: number[];
}

const CARD_SIZE_MIN = 300;
const CARD_SIZE_MAX = 1600;
const CARD_SIZE_STEP = 50;
/** A card size chosen in this browser, kept until Fit clears it. */
const CARD_SIZE_KEY = 'psf-guard.stacks.card-size';
/** A row this wide holds two fitted cards; the CSS draws the same line. */
const TWO_UP_WIDTH = 2016;

function storedCardSize(): number | null {
  try {
    const value = Number(window.localStorage.getItem(CARD_SIZE_KEY));
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch {
    return null;
  }
}

function storeCardSize(size: number | null) {
  try {
    if (size === null) window.localStorage.removeItem(CARD_SIZE_KEY);
    else window.localStorage.setItem(CARD_SIZE_KEY, String(size));
  } catch {
    // Private windows and blocked storage keep the size for this page only.
  }
}

const clampCardSize = (size: number) => Math.min(CARD_SIZE_MAX, Math.max(CARD_SIZE_MIN, size));

/**
 * Stacks: PSF Guard's stack previews and their color, and the stacks WBPP
 * made, for the project in scope. Beside Images and Sequence because a stack
 * is another way to look at the same frames. It stacks every frame in scope
 * unless Images handed over a selection.
 */
export default function StacksView() {
  const { dbId, projectId, targetId, mosaic: mosaicScope } = useDbProjectTarget();
  const { data: mosaic } = useProjectMosaic(mosaicScope ? dbId : null, mosaicScope ? projectId : null);
  const navigate = useNavigate();
  const location = useLocation();
  const latestParams = useLatestSearchParams();
  const handed = (location.state as StackSelectionState | null)?.stackImageIds;
  // How wide the cards are: a size in the link, else one chosen earlier in
  // this browser, else they fill the row (one card, or two on a wide screen).
  const { getNumberParam, updateParams } = useUrlParams();
  const requestedSize = getNumberParam('cardsize');
  const [storedSize, setStoredSize] = useState(storedCardSize);
  const chosenSize = requestedSize !== null && Number.isFinite(requestedSize) ? requestedSize : storedSize;
  const cardSize = chosenSize === null ? null : clampCardSize(chosenSize);
  const chooseCardSize = (size: number | null) => {
    updateParams({ cardsize: size });
    storeCardSize(size);
    setStoredSize(size);
  };
  // Where the slider sits while the cards fit: about as wide as they are.
  const viewRef = useRef<HTMLDivElement>(null);
  const [viewWidth, setViewWidth] = useState(0);
  useEffect(() => {
    const view = viewRef.current;
    if (!view || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(([entry]) => setViewWidth(entry.contentRect.width));
    observer.observe(view);
    return () => observer.disconnect();
  }, []);
  const fittedSize = clampCardSize(
    Math.round((viewWidth >= TWO_UP_WIDTH ? (viewWidth - 16) / 2 : viewWidth) / CARD_SIZE_STEP) * CARD_SIZE_STEP
  );
  const queryClient = useQueryClient();
  const [wbppOpen, setWbppOpen] = useState(false);
  const access = useAccess();

  // The same query as Images, so moving between the two reads one cache.
  // Nights as the view splits them, the server's setting or the link's.
  const nightSplit = useNightSplit();
  const { data: images = [], isLoading } = useQuery({
    queryKey: ['db', dbId, 'all-images', projectId, targetId, nightSplit.queryKey],
    queryFn: () =>
      apiClient.getImages(dbId!, {
        project_id: projectId || undefined,
        target_id: targetId || undefined,
        limit: 10000,
        nights: nightSplit.override ?? undefined,
      }),
    enabled: !!dbId && projectId != null,
    // A new night split keeps the old frames on screen until its own
    // arrive, so the grid neither empties nor loses its place.
    placeholderData: keepAcrossNightSplit(['db', dbId, 'all-images', projectId, targetId]),
    refetchInterval: 30000,
  });
  const selection = useMemo(() => {
    if (!handed || handed.length < 2) return null;
    const wanted = new Set(handed);
    const chosen = images.filter((image) => wanted.has(image.id));
    return chosen.length >= 2 ? chosen : null;
  }, [handed, images]);
  const openImage = useCallback(
    (imageId: number) => navigate(imageDetailPath(imageId, latestParams(), 'grid')),
    [latestParams, navigate]
  );

  // WBPP runs go in the folders the PixInsight settings define, so an
  // editor runs them; only choosing another folder takes database
  // management, and the server says so.
  const canManage = access.canWrite;
  const { status: wbppStatus } = useWbppRun(canManage ? dbId : null);
  const { data: exportSettings } = useQuery({
    queryKey: ['export-settings'],
    queryFn: apiClient.getExportSettings,
    staleTime: 5 * 60 * 1000,
    enabled: canManage,
  });
  const wbppState =
    projectId != null ? describeWbppRunForProject(wbppStatus, projectId, targetId) : null;
  // How the finished run's masters came in as stacks. When the import
  // settles, the stacks and the color channels it added are refetched.
  const runStacks = wbppState?.tone === 'done' ? wbppStatus?.progress.stacks ?? null : null;
  const settled = runStacks && runStacks.state !== 'importing'
    ? `${wbppStatus?.progress.started_at}:${runStacks.state}`
    : null;
  const lastSettled = useRef<string | null>(null);
  useEffect(() => {
    if (settled && lastSettled.current !== null && settled !== lastSettled.current && dbId) {
      queryClient.invalidateQueries({ queryKey: ['db', dbId] });
    }
    if (settled) lastSettled.current = settled;
    else if (lastSettled.current === null) lastSettled.current = '';
  }, [settled, dbId, queryClient]);
  // WBPP integrates a project's targets together, so a project-wide run
  // over several targets cannot come back as stacks.
  const severalTargets =
    targetId == null && new Set(images.map((image) => image.target_id)).size > 1;
  const scopeLabel =
    (targetId != null ? images.find((image) => image.target_id === targetId)?.target_name : null) ??
    images[0]?.project_name ??
    `Project ${projectId}`;

  if (!dbId || projectId == null) {
    return (
      <div className="stacks-view stacks-empty">
        <h1>Stacks</h1>
        <p className="muted">Choose a project in the header to stack its frames.</p>
      </div>
    );
  }
  if (isLoading) return <div className="loading">Loading images...</div>;

  return (
    <div className="stacks-view" ref={viewRef}>
      {/* Whether exposure lengths stack apart is a choice about stacks first. */}
      <div className="stacks-toolbar">
        {/* A phone fits one card to the row whatever the size. */}
        <span className="stacks-card-size">
          <ThumbnailSizeControl
            id="stacks-card-size"
            label="Card size"
            value={cardSize ?? fittedSize}
            valueText={cardSize === null ? 'Fit' : undefined}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            onChange={chooseCardSize}
          />
          {cardSize !== null && (
            <button type="button" className="link-button" title="Fill the row: one card, or two on a wide screen" onClick={() => chooseCardSize(null)}>
              Fit
            </button>
          )}
        </span>
        <ProjectExposureGrouping
          key={`${dbId}:${projectId}`}
          dbId={dbId}
          projectId={projectId}
          canManage={access.canWrite}
        />
      </div>
      {selection && (
        <p className="stacks-selection" role="status">
          Stacking the {selection.length} images selected in Images.{' '}
          <button
            type="button"
            className="link-button"
            onClick={() => navigate(`${location.pathname}?${latestParams()}`, { replace: true, state: null })}
          >
            Stack every image instead
          </button>
        </p>
      )}
      {mosaic && <MosaicStacks mosaic={mosaic} />}
      <StackPreviewPanel
        dbId={dbId}
        projectId={projectId}
        images={selection ?? images}
        selectionSource={selection ? 'selected' : 'scope'}
        cardSize={cardSize}
        onOpenImage={openImage}
        targetId={targetId}
        actions={
          canManage && (
            <button
              type="button"
              className={`stack-preview-wbpp${wbppState ? ` wbpp-state-${wbppState.tone}` : ''}`}
              disabled={!wbppState && severalTargets}
              title={
                wbppState
                  ? "Open this project's WBPP run"
                  : severalTargets
                    ? 'Choose a target in the header first. WBPP combines targets within a project, so PSF Guard cannot import their masters as separate target stacks.'
                    : "Stack with PixInsight's WBPP on the server. Its master lights appear here as stacks."
              }
              onClick={() => setWbppOpen(true)}
            >
              {wbppState ? wbppState.label : 'Stack in WBPP'}
            </button>
          )
        }
      />
      <WbppStacks
        dbId={dbId}
        projectId={projectId}
        targetId={targetId}
        canImport={canManage}
        lastRun={runStacks}
      />
      {wbppOpen && (
        <WbppRunDialog
          request={{
            dbId,
            scope: targetId != null ? { project_id: projectId, target_id: targetId } : { project_id: projectId },
            label: scopeLabel,
          }}
          defaultOptions={exportSettings?.wbpp ?? DEFAULT_WBPP_OPTIONS}
          onClose={() => setWbppOpen(false)}
        />
      )}
    </div>
  );
}
