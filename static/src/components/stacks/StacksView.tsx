import { useCallback, useMemo } from 'react';
import { useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useDbProjectTarget, useGridState } from '../../hooks/useUrlState';
import { useAccess } from '../../auth/access';
import ProjectExposureGrouping from '../ProjectExposureGrouping';
import { imageDetailPath } from '../../utils/imageDetailRoutes';
import StackPreviewPanel from '../StackPreviewPanel';
import WbppStacks from './WbppStacks';
import './stacks.css';

/** What Images hands over when a person stacks a selection. */
export interface StackSelectionState {
  stackImageIds?: number[];
}

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
  // The grid's zoom widens the stack cards here too, as it did in Images.
  const { imageSize } = useGridState();
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
        imageSize={imageSize}
        onOpenImage={openImage}
        targetId={targetId}
      />
      <WbppStacks dbId={dbId} projectId={projectId} targetId={targetId} canImport={canManage} />
    </div>
  );
}
