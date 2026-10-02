import { useCallback, useMemo, useState } from 'react';
import { useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useDbProjectTarget, useUrlParams } from '../../hooks/useUrlState';
import { useAccess } from '../../auth/access';
import ProjectExposureGrouping from '../ProjectExposureGrouping';
import { imageDetailPath } from '../../utils/imageDetailRoutes';
import StackPreviewPanel, { DEFAULT_STACK_CARD_SIZE } from '../StackPreviewPanel';
import ThumbnailSizeControl from '../ThumbnailSizeControl';
import WbppRunDialog from '../WbppRunDialog';
import { describeWbppRunForProject, useWbppRun } from '../../hooks/useWbppRun';
import { DEFAULT_WBPP_OPTIONS } from '../../api/types';
import WbppStacks from './WbppStacks';
import './stacks.css';

/** What Images hands over when a person stacks a selection. */
export interface StackSelectionState {
  stackImageIds?: number[];
}

const CARD_SIZE_MIN = 300;
const CARD_SIZE_MAX = 1600;
const CARD_SIZE_STEP = 50;

/**
 * Stacks: PSF Guard's stack previews and their color, and the stacks WBPP
 * made, for the project in scope. Beside Images and Sequence because a stack
 * is another way to look at the same frames. It stacks every frame in scope
 * unless Images handed over a selection.
 */
export default function StacksView() {
  const { dbId, projectId, targetId } = useDbProjectTarget();
  const navigate = useNavigate();
  const location = useLocation();
  const [searchParams] = useSearchParams();
  const handed = (location.state as StackSelectionState | null)?.stackImageIds;
  // How wide the cards are; in the URL so a reload keeps the layout.
  const { getNumberParam, updateParams } = useUrlParams();
  const cardSize = Math.min(
    CARD_SIZE_MAX,
    Math.max(CARD_SIZE_MIN, getNumberParam('cardsize') ?? DEFAULT_STACK_CARD_SIZE)
  );
  const [wbppOpen, setWbppOpen] = useState(false);
  const access = useAccess();
  const { data: serverInfo } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });

  // The same query as Images, so moving between the two reads one cache.
  const { data: images = [], isLoading } = useQuery({
    queryKey: ['db', dbId, 'all-images', projectId, targetId],
    queryFn: () =>
      apiClient.getImages(dbId!, {
        project_id: projectId || undefined,
        target_id: targetId || undefined,
        limit: 10000,
      }),
    enabled: !!dbId && projectId != null,
    refetchInterval: 30000,
  });
  const selection = useMemo(() => {
    if (!handed || handed.length < 2) return null;
    const wanted = new Set(handed);
    const chosen = images.filter((image) => wanted.has(image.id));
    return chosen.length >= 2 ? chosen : null;
  }, [handed, images]);
  const openImage = useCallback(
    (imageId: number) => navigate(imageDetailPath(imageId, searchParams, 'grid')),
    [navigate, searchParams]
  );

  const canManage = access.canWrite && !!serverInfo?.allow_database_management;
  // WBPP runs write files, so they sit behind database management, as in
  // the Library.
  const { status: wbppStatus } = useWbppRun(canManage ? dbId : null);
  const { data: exportSettings } = useQuery({
    queryKey: ['export-settings'],
    queryFn: apiClient.getExportSettings,
    staleTime: 5 * 60 * 1000,
    enabled: canManage,
  });
  const wbppState = projectId != null ? describeWbppRunForProject(wbppStatus, projectId) : null;
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
    <div className="stacks-view">
      {/* Whether exposure lengths stack apart is a choice about stacks first. */}
      <div className="stacks-toolbar">
        <ThumbnailSizeControl
          id="stacks-card-size"
          value={cardSize}
          min={CARD_SIZE_MIN}
          max={CARD_SIZE_MAX}
          step={CARD_SIZE_STEP}
          onChange={(size) => updateParams({ cardsize: size })}
        />
        <ProjectExposureGrouping
          key={`${dbId}:${projectId}`}
          dbId={dbId}
          projectId={projectId}
          canManage={canManage}
        />
      </div>
      {selection && (
        <p className="stacks-selection" role="status">
          Stacking the {selection.length} images selected in Images.{' '}
          <button
            type="button"
            className="link-button"
            onClick={() => navigate(`${location.pathname}${location.search}`, { replace: true, state: null })}
          >
            Stack every image instead
          </button>
        </p>
      )}
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
              title={
                wbppState
                  ? "Open this project's WBPP run"
                  : "Stack in PixInsight's WBPP on the server; its master lights come back here as stacks"
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
        lastRunFinished={wbppState?.tone === 'done'}
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
